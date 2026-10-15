import { videoInfo } from "./youtube.js";

const TRACE = process.env.SWEET_MEDIA_TRACE != null;
const MiB = 1024 * 1024;
const RANGE_BLOCK = 512 * 1024;

export function isYoutubeSource(source) {
  try {
    const hostname = new URL(source).hostname.toLowerCase().replace(/^www\./, "");
    return hostname === "youtube.com" || hostname.endsWith(".youtube.com") || hostname === "youtu.be";
  } catch {
    return false;
  }
}

function trace(message, details = {}) {
  if (TRACE) console.error(`[sweet-js-stream ${new Date().toISOString()}] ${message}`, details);
}

const u16 = (data, at) => (data[at] << 8) | data[at + 1];
const u32 = (data, at) => ((data[at] * 0x1000000) + (data[at + 1] << 16) + (data[at + 2] << 8) + data[at + 3]);
const u64 = (data, at) => u32(data, at) * 0x100000000 + u32(data, at + 4);
const text = (data, at, length) => String.fromCharCode(...data.subarray(at, at + length));

function atomAt(data, at, limit = data.length) {
  if (at + 8 > limit) return null;
  const size32 = u32(data, at);
  const type = text(data, at + 4, 4);
  let size = size32;
  let header = 8;
  if (size32 === 1) {
    if (at + 16 > limit) return null;
    size = u64(data, at + 8);
    header = 16;
  } else if (size32 === 0) {
    size = limit - at;
  }
  if (!Number.isSafeInteger(size) || size < header || at + size > limit) return null;
  return { type, start: at, size, header, payload: at + header, end: at + size };
}

function children(data, parent, start = parent.payload, end = parent.end) {
  const result = [];
  for (let at = start; at + 8 <= end;) {
    const box = atomAt(data, at, end);
    if (!box) break;
    result.push(box);
    at = box.end;
  }
  return result;
}

function child(data, parent, type) {
  return children(data, parent).find((box) => box.type === type) || null;
}

function descendant(data, parent, type) {
  for (const box of children(data, parent)) {
    if (box.type === type) return box;
    const found = descendant(data, box, type);
    if (found) return found;
  }
  return null;
}

function locateMoov(data) {
  for (let at = 0; at + 8 <= data.length;) {
    const box = atomAt(data, at);
    if (!box) break;
    if (box.type === "moov") return box;
    at = box.end;
  }
  // A suffix range does not start on a top-level atom boundary. Validate the
  // size/type pair before accepting a match from inside that range.
  for (let at = 4; at + 4 <= data.length; at += 1) {
    if (text(data, at, 4) !== "moov") continue;
    const start = at - 4;
    const box = atomAt(data, start);
    if (box?.type === "moov") return box;
    if (start >= 8 && u32(data, start) === 1) {
      const extended = atomAt(data, start);
      if (extended?.type === "moov") return extended;
    }
  }
  return null;
}

function findSampleEntry(data, stsd) {
  if (!stsd) throw new Error("MP4 is missing stsd");
  const entry = atomAt(data, stsd.payload + 8, stsd.end);
  if (!entry) throw new Error("MP4 has an invalid sample description");
  return entry;
}

function videoConfig(data, entry) {
  const childrenStart = entry.payload + 78;
  const codecBox = children(data, entry, childrenStart).find((box) => ["avcC", "hvcC", "av1C"].includes(box.type));
  if (!codecBox) throw new Error(`MP4 ${entry.type} sample entry has no decoder configuration`);
  const config = data.subarray(codecBox.payload, codecBox.end);
  if (codecBox.type === "avcC") {
    const nalLengthSize = (config[4] & 3) + 1;
    let at = 6;
    const units = [];
    const spsCount = config[5] & 31;
    for (let i = 0; i < spsCount; i += 1) {
      const size = u16(config, at); at += 2;
      units.push(config.subarray(at, at + size)); at += size;
    }
    const ppsCount = config[at++];
    for (let i = 0; i < ppsCount; i += 1) {
      const size = u16(config, at); at += 2;
      units.push(config.subarray(at, at + size)); at += size;
    }
    return { codec: "h264", nalLengthSize, private: annexB(units) };
  }
  if (codecBox.type === "hvcC") {
    const nalLengthSize = (config[21] & 3) + 1;
    const arrays = config[22];
    let at = 23;
    const units = [];
    for (let i = 0; i < arrays; i += 1) {
      at += 1;
      const count = u16(config, at); at += 2;
      for (let j = 0; j < count; j += 1) {
        const size = u16(config, at); at += 2;
        units.push(config.subarray(at, at + size)); at += size;
      }
    }
    return { codec: "hevc", nalLengthSize, private: annexB(units) };
  }
  return { codec: "av1", nalLengthSize: 0, private: config.subarray(4) };
}

function annexB(nals) {
  const output = [];
  for (const nal of nals) output.push(Buffer.from([0, 0, 0, 1]), Buffer.from(nal));
  return Buffer.concat(output);
}

function parseRunTable(data, box, signed = false) {
  if (!box) return [];
  const count = u32(data, box.payload + 4);
  const result = [];
  let at = box.payload + 8;
  for (let i = 0; i < count && at + 8 <= box.end; i += 1) {
    result.push([u32(data, at), signed ? (u32(data, at + 4) | 0) : u32(data, at + 4)]);
    at += 8;
  }
  return result;
}

function readDescriptor(data, at, end) {
  if (at >= end) return null;
  const tag = data[at++];
  let length = 0;
  let byte;
  let count = 0;
  do {
    if (at >= end || count++ >= 4) return null;
    byte = data[at++];
    length = (length << 7) | (byte & 0x7f);
  } while (byte & 0x80);
  if (at + length > end) return null;
  return { tag, payload: at, end: at + length, next: at + length };
}

function audioSpecificConfig(data, esds) {
  let descriptor = readDescriptor(data, esds.payload + 4, esds.end);
  if (!descriptor || descriptor.tag !== 0x03) return null;
  let at = descriptor.payload + 3;
  const flags = data[descriptor.payload + 2];
  if (flags & 0x80) at += 2;
  if (flags & 0x40) at += 1 + (data[at] || 0);
  if (flags & 0x20) at += 2;
  while (at < descriptor.end) {
    const nested = readDescriptor(data, at, descriptor.end);
    if (!nested) break;
    if (nested.tag === 0x04) {
      let childAt = nested.payload + 13;
      while (childAt < nested.end) {
        const config = readDescriptor(data, childAt, nested.end);
        if (!config) break;
        if (config.tag === 0x05) return data.slice(config.payload, config.end);
        childAt = config.next;
      }
    }
    at = nested.next;
  }
  return null;
}

function parseSampleTable(data, stbl, timescale) {
  const stsz = child(data, stbl, "stsz");
  if (!stsz) throw new Error("MP4 is missing stsz");
  const defaultSize = u32(data, stsz.payload + 4);
  const sampleCount = u32(data, stsz.payload + 8);
  // Fragmented MP4 keeps its media samples in moof/trun boxes instead of the
  // classic stsz/stco tables. The track configuration still lives in moov.
  if (sampleCount === 0) return { samples: [], duration: 0 };
  const sizes = new Array(sampleCount);
  if (defaultSize) sizes.fill(defaultSize);
  else for (let i = 0, at = stsz.payload + 12; i < sampleCount; i += 1, at += 4) sizes[i] = u32(data, at);

  const stco = child(data, stbl, "stco");
  const co64 = child(data, stbl, "co64");
  const offsetsBox = stco || co64;
  if (!offsetsBox) throw new Error("MP4 is missing chunk offsets");
  const chunkCount = u32(data, offsetsBox.payload + 4);
  const chunkOffsets = new Array(chunkCount);
  for (let i = 0, at = offsetsBox.payload + 8; i < chunkCount; i += 1) {
    chunkOffsets[i] = stco ? u32(data, at) : u64(data, at);
    at += stco ? 4 : 8;
  }
  const stsc = child(data, stbl, "stsc");
  const stscCount = stsc ? u32(data, stsc.payload + 4) : 0;
  const chunks = [];
  for (let i = 0, at = stsc.payload + 8; i < stscCount; i += 1, at += 12) {
    chunks.push({ first: u32(data, at), perChunk: u32(data, at + 4) });
  }
  const sampleOffsets = new Array(sampleCount);
  let sample = 0;
  for (let i = 0; i < chunkOffsets.length && sample < sampleCount; i += 1) {
    const chunkNumber = i + 1;
    let map = chunks[0];
    for (const candidate of chunks) {
      if (candidate.first > chunkNumber) break;
      map = candidate;
    }
    let at = chunkOffsets[i];
    for (let j = 0; j < (map?.perChunk || 1) && sample < sampleCount; j += 1) {
      sampleOffsets[sample] = at;
      at += sizes[sample];
      sample += 1;
    }
  }
  if (sample !== sampleCount) throw new Error("MP4 sample table is incomplete");

  const stts = parseRunTable(data, child(data, stbl, "stts"));
  const cttsBox = child(data, stbl, "ctts");
  const ctts = parseRunTable(data, cttsBox, cttsBox && data[cttsBox.payload] === 1);
  const syncBox = child(data, stbl, "stss");
  const sync = syncBox ? new Set(Array.from({ length: u32(data, syncBox.payload + 4) }, (_, i) => u32(data, syncBox.payload + 8 + i * 4) - 1)) : null;
  const samples = new Array(sampleCount);
  let dts = 0;
  let sampleIndex = 0;
  let cttsIndex = 0;
  let cttsUsed = 0;
  for (const [count, delta] of stts) {
    for (let i = 0; i < count && sampleIndex < sampleCount; i += 1) {
      while (cttsIndex < ctts.length && cttsUsed >= ctts[cttsIndex][0]) {
        cttsIndex += 1; cttsUsed = 0;
      }
      const composition = ctts[cttsIndex]?.[1] || 0;
      samples[sampleIndex] = {
        offset: sampleOffsets[sampleIndex],
        size: sizes[sampleIndex],
        timestamp: (dts + composition) / timescale,
        duration: delta / timescale,
        keyframe: sync ? sync.has(sampleIndex) : true,
      };
      dts += delta;
      sampleIndex += 1;
      cttsUsed += 1;
    }
  }
  if (sampleIndex !== sampleCount) throw new Error("MP4 decode-time table is incomplete");
  return { samples, duration: dts / timescale };
}

function parseTrack(data, trak) {
  const mdia = child(data, trak, "mdia");
  const hdlr = child(data, mdia, "hdlr");
  const handler = text(data, hdlr.payload + 8, 4);
  if (handler !== "vide" && handler !== "soun") return null;
  const mdhd = child(data, mdia, "mdhd");
  const version = data[mdhd.payload];
  const timescale = u32(data, mdhd.payload + (version === 1 ? 20 : 12));
  const tkhd = child(data, trak, "tkhd");
  const trackId = u32(data, tkhd.payload + (data[tkhd.payload] === 1 ? 20 : 12));
  const minf = child(data, mdia, "minf");
  const stbl = child(data, minf, "stbl");
  const stsd = child(data, stbl, "stsd");
  const entry = findSampleEntry(data, stsd);
  const table = parseSampleTable(data, stbl, timescale);
  if (handler === "soun") {
    if (entry.type !== "mp4a") return null;
    const childrenStart = entry.payload + 28;
    const esds = children(data, entry, childrenStart).find((box) => box.type === "esds");
    if (!esds) return null;
    const extraData = audioSpecificConfig(data, esds);
    if (!extraData) throw new Error("MP4 AAC track has no AudioSpecificConfig");
    const sampleRate = u32(data, entry.payload + 24) >>> 16;
    const channels = u16(data, entry.payload + 16);
    return { kind: "audio", trackId, codec: "aac", extraData, sampleRate, channels, timeScale: timescale, ...table };
  }
  const config = videoConfig(data, entry);
  const entryChildren = children(data, entry, entry.payload + 78);
  const width = u16(data, entry.payload + 24);
  const height = u16(data, entry.payload + 26);
  return { kind: "video", trackId, timeScale: timescale, ...config, ...table, duration: table.duration, width, height };
}

function parseMoov(data, atom) {
  const tracks = children(data, atom).filter((box) => box.type === "trak");
  let video = null;
  let audio = null;
  for (const trak of tracks) {
    const parsed = parseTrack(data, trak);
    if (parsed?.kind === "video") video = parsed;
    if (parsed?.kind === "audio") audio = parsed;
  }
  if (!video && !audio) throw new Error("MP4 has no readable audio or video track");
  const trex = new Map();
  const mvex = child(data, atom, "mvex");
  if (mvex) {
    for (const box of children(data, mvex).filter((entry) => entry.type === "trex")) {
      const id = u32(data, box.payload + 4);
      trex.set(id, {
        duration: u32(data, box.payload + 12),
        size: u32(data, box.payload + 16),
        flags: u32(data, box.payload + 20),
      });
    }
  }
  return { video, audio, trex };
}

function topLevelHeader(data, at) {
  if (at + 8 > data.length) return null;
  const size32 = u32(data, at);
  const type = text(data, at + 4, 4);
  const header = size32 === 1 ? 16 : 8;
  if (at + header > data.length) return null;
  const size = size32 === 1 ? u64(data, at + 8) : size32 === 0 ? Infinity : size32;
  if (!Number.isSafeInteger(size) && size !== Infinity || size < header) return null;
  return { type, size, header };
}

function signed32(value) {
  return value | 0;
}

function parseMovieFragment(data, atom, absoluteOffset, mediaDataOffset, track, defaults, nextDecodeTime) {
  const samples = [];
  for (const traf of children(data, atom).filter((box) => box.type === "traf")) {
    const tfhd = child(data, traf, "tfhd");
    if (!tfhd) continue;
    const flags = (data[tfhd.payload + 1] << 16) | (data[tfhd.payload + 2] << 8) | data[tfhd.payload + 3];
    const trackId = u32(data, tfhd.payload + 4);
    if (trackId !== track.trackId) continue;
    let at = tfhd.payload + 8;
    let baseDataOffset = absoluteOffset;
    if (flags & 0x000001) { baseDataOffset = u64(data, at); at += 8; }
    if (flags & 0x000002) at += 4; // sample_description_index
    const defaultDuration = flags & 0x000008 ? u32(data, at) : defaults?.duration || 0;
    if (flags & 0x000008) at += 4;
    const defaultSize = flags & 0x000010 ? u32(data, at) : defaults?.size || 0;
    if (flags & 0x000010) at += 4;
    const defaultFlags = flags & 0x000020 ? u32(data, at) : defaults?.flags || 0;
    let decodeTime = nextDecodeTime;
    const tfdt = child(data, traf, "tfdt");
    if (tfdt) decodeTime = data[tfdt.payload] === 1 ? u64(data, tfdt.payload + 4) : u32(data, tfdt.payload + 4);
    let dataCursor = mediaDataOffset;
    for (const trun of children(data, traf).filter((box) => box.type === "trun")) {
      const version = data[trun.payload];
      const runFlags = (data[trun.payload + 1] << 16) | (data[trun.payload + 2] << 8) | data[trun.payload + 3];
      const count = u32(data, trun.payload + 4);
      let p = trun.payload + 8;
      let firstFlags = null;
      if (runFlags & 0x000001) {
        dataCursor = baseDataOffset + signed32(u32(data, p));
        p += 4;
      }
      if (runFlags & 0x000004) { firstFlags = u32(data, p); p += 4; }
      for (let i = 0; i < count; i += 1) {
        const duration = runFlags & 0x000100 ? u32(data, p) : defaultDuration;
        if (runFlags & 0x000100) p += 4;
        const size = runFlags & 0x000200 ? u32(data, p) : defaultSize;
        if (runFlags & 0x000200) p += 4;
        const sampleFlags = runFlags & 0x000400 ? u32(data, p) : i === 0 && firstFlags != null ? firstFlags : defaultFlags;
        if (runFlags & 0x000400) p += 4;
        const hasSampleFlags = !!(runFlags & 0x000400) || (i === 0 && firstFlags != null) || !!(flags & 0x000020) || defaults != null;
        let composition = 0;
        if (runFlags & 0x000800) {
          const value = u32(data, p);
          composition = version === 1 ? signed32(value) : value;
          p += 4;
        }
        if (!size || !duration) throw new Error("fragmented MP4 sample is missing size or duration");
        samples.push({
          offset: dataCursor,
          size,
          timestamp: (decodeTime + composition) / track.timeScale,
          duration: duration / track.timeScale,
          keyframe: track.kind === "audio" || (hasSampleFlags ? (sampleFlags & 0x00010000) === 0 : i === 0),
        });
        decodeTime += duration;
        dataCursor += size;
      }
    }
    return { samples, nextDecodeTime: decodeTime };
  }
  return { samples, nextDecodeTime };
}

async function indexFragments(format, metadata, track, defaults, signal) {
  const samples = [];
  let offset = 0;
  let nextDecodeTime = 0;
  let data = metadata.start === 0 ? metadata.data : null;
  let dataStart = data ? 0 : -1;
  while (offset + 8 <= metadata.total) {
    if (!data || offset < dataStart || offset + 8 > dataStart + data.length) {
      dataStart = offset;
      const end = Math.min(metadata.total - 1, offset + RANGE_BLOCK - 1);
      const range = await fetchRange(format.url, offset, end, format.http_headers || {}, signal);
      if (range.complete) {
        data = range.data;
        dataStart = 0;
      } else {
        data = range.data;
        dataStart = range.start;
      }
    }
    const local = offset - dataStart;
    const header = topLevelHeader(data, local);
    if (!header) throw new Error(`invalid MP4 top-level box header at byte ${offset}`);
    const boxOffset = offset;
    const boxSize = header.size === Infinity ? metadata.total - offset : header.size;
    if (header.type === "moof") {
      let moofData = data;
      let moofLocal = local;
      if (local + boxSize > data.length) {
        const range = await fetchRange(format.url, boxOffset, boxOffset + boxSize - 1, format.http_headers || {}, signal);
        moofData = range.data;
        moofLocal = boxOffset - range.start;
      }
      const moof = atomAt(moofData, moofLocal, moofLocal + boxSize);
      if (!moof) throw new Error(`could not read MP4 movie fragment at byte ${boxOffset}`);
      let following = null;
      if (boxOffset + boxSize + 8 <= metadata.total) {
        const followingHeader = await fetchRange(format.url, boxOffset + boxSize, boxOffset + boxSize + 7, format.http_headers || {}, signal);
        following = topLevelHeader(followingHeader.data, boxOffset + boxSize - followingHeader.start);
      }
      const mediaOffset = following?.type === "mdat" ? boxOffset + boxSize + following.header : boxOffset + boxSize;
      const parsed = parseMovieFragment(moofData, moof, boxOffset, mediaOffset, track, defaults, nextDecodeTime);
      samples.push(...parsed.samples);
      nextDecodeTime = parsed.nextDecodeTime;
    }
    if (!Number.isSafeInteger(boxSize) || boxSize <= 0) break;
    offset = boxOffset + boxSize;
  }
  return samples;
}

async function fetchRange(url, start, end, headers, signal) {
  const response = await fetch(url, {
    headers: { ...headers, Range: `bytes=${start}-${end}` },
    signal,
  });
  if (response.status !== 206 && response.status !== 200) {
    throw new Error(`media range request failed: HTTP ${response.status}`);
  }
  const requestedLength = end - start + 1;
  const contentLength = Number(response.headers.get("content-length"));
  // Do not turn a byte-range request into a whole-video download. Small files
  // can legitimately fit in the requested metadata window, but a large 200
  // response means the server ignored Range and cannot support our seekable
  // streaming path.
  if (response.status === 200 && (!Number.isFinite(contentLength) || contentLength > Math.max(requestedLength, RANGE_BLOCK))) {
    await response.body?.cancel();
    throw new Error("media server ignored byte ranges; seekable streaming requires HTTP 206 responses");
  }
  const data = new Uint8Array(await response.arrayBuffer());
  const contentRange = response.headers.get("content-range") || "";
  const match = contentRange.match(/bytes\s+(\d+)-(\d+)\/(\d+|\*)/i);
  return {
    data,
    start: response.status === 206 && match ? Number(match[1]) : 0,
    total: match && match[3] !== "*" ? Number(match[3]) : Number(response.headers.get("content-length")) || data.byteLength,
    complete: response.status === 200,
  };
}

async function readMoov(format) {
  const headers = format.http_headers || {};
  const probe = await fetchRange(format.url, 0, 0, headers);
  const total = probe.total > 1 ? probe.total : Number(format.filesize || format.filesize_approx);
  const head = probe.complete ? probe : await fetchRange(format.url, 0, Math.min(total - 1, 512 * 1024 - 1), headers);
  let atom = locateMoov(head.data);
  if (atom) return { data: head.data, atom, base: head.start, total, complete: head.complete };
  if (head.complete) throw new Error("Could not find MP4 moov header");
  for (const tailSize of [512 * 1024, 2 * MiB, 8 * MiB, 32 * MiB]) {
    const start = Math.max(0, total - tailSize);
    const tail = await fetchRange(format.url, start, total - 1, headers);
    atom = locateMoov(tail.data);
    if (atom) return { data: tail.data, atom, base: tail.start, total, complete: tail.complete };
    if (tail.complete) throw new Error("Could not find MP4 moov header");
    if (!start) break;
  }
  throw new Error("MP4 moov box is larger than the supported metadata range");
}

function sampleBytes(data, nalLengthSize, codec) {
  if (codec === "av1") return Buffer.from(data);
  const units = [];
  for (let at = 0; at + nalLengthSize <= data.length;) {
    let size = 0;
    for (let i = 0; i < nalLengthSize; i += 1) size = size * 256 + data[at + i];
    at += nalLengthSize;
    if (!size || at + size > data.length) throw new Error("MP4 sample contains an invalid NAL length");
    units.push(data.subarray(at, at + size));
    at += size;
  }
  return annexB(units);
}

function chooseFormat(info) {
  const maxHeight = process.platform === "android" ? 480 : 720;
  const formats = (info.formats || []).filter((f) => f.url && f.ext === "mp4" && /^avc1|^hvc1|^hev1|^av01/.test(f.vcodec || "") && f.protocol !== "m3u8" && f.protocol !== "m3u8_native" && (!f.height || f.height <= maxHeight));
  const codecRank = (format) => /^avc1/.test(format.vcodec || "") ? 3 : /^hvc1|^hev1/.test(format.vcodec || "") ? 2 : 1;
  formats.sort((a, b) => codecRank(b) - codecRank(a) || (b.height || 0) - (a.height || 0) || Number(!!b.acodec && b.acodec !== "none") - Number(!!a.acodec && a.acodec !== "none") || (b.tbr || 0) - (a.tbr || 0));
  if (!formats.length) throw new Error("YouTube has no supported MP4 video format");
  const video = formats[0];
  const audio = video.acodec && video.acodec !== "none"
    ? null
    : (info.formats || [])
      .filter((f) => f.url && f.ext === "m4a" && f.acodec?.startsWith("mp4a") && (!f.vcodec || f.vcodec === "none"))
      .sort((a, b) => (b.abr || 0) - (a.abr || 0))[0] || null;
  return { video, audio };
}

function batchEnd(samples, first) {
  let end = samples[first].offset + samples[first].size;
  let next = first + 1;
  while (next < samples.length && next - first < 128) {
    const candidateEnd = samples[next].offset + samples[next].size;
    if (candidateEnd - samples[first].offset > RANGE_BLOCK || samples[next].offset - end > 128 * 1024) break;
    end = candidateEnd;
    next += 1;
  }
  return { next, end };
}

function syncSampleAt(samples, seconds) {
  let chosen = 0;
  let chosenTime = -Infinity;
  for (let i = 0; i < samples.length; i += 1) {
    if (samples[i].timestamp <= seconds && samples[i].keyframe && samples[i].timestamp >= chosenTime) {
      chosen = i;
      chosenTime = samples[i].timestamp;
    }
  }
  return chosen;
}

export async function createYoutubeVideoStream(source, native, isCurrent = () => true) {
  trace("yt-dlp metadata lookup begin", { source: new URL(source).host });
  const info = await videoInfo(source);
  const { video: format, audio: separateAudio } = chooseFormat(info);
  trace("selected MP4 format", { format: format.format_id, codec: format.vcodec, height: format.height, size: format.filesize || format.filesize_approx, separateAudio: !!separateAudio });
  const [metadata, audioMetadata] = await Promise.all([
    readMoov(format),
    separateAudio ? readMoov(separateAudio) : Promise.resolve(null),
  ]);
  const tracks = parseMoov(metadata.data, metadata.atom);
  const stream = tracks.video;
  if (!stream) throw new Error("Selected YouTube format has no supported video track");
  const separateTracks = audioMetadata ? parseMoov(audioMetadata.data, audioMetadata.atom) : null;
  const audio = tracks.audio || separateTracks?.audio || null;
  if (stream.samples.length === 0) {
    stream.samples = await indexFragments(format, metadata, stream, tracks.trex.get(stream.trackId));
  }
  if (audio?.samples.length === 0) {
    const audioFormat = tracks.audio ? format : separateAudio;
    const audioInfo = tracks.audio ? metadata : audioMetadata;
    const audioDefaults = tracks.audio ? tracks.trex.get(audio.trackId) : separateTracks?.trex.get(audio.trackId);
    audio.samples = await indexFragments(audioFormat, audioInfo, audio, audioDefaults);
  }
  if (stream.samples.length === 0) throw new Error("MP4 contains no indexed video samples");
  const duration = Number(info.duration) || stream.duration;
  if (!isCurrent()) throw new Error("video source changed during stream setup");
  native.configureVideoStream(stream.codec, Buffer.from(stream.private), duration, stream.width || format.width || 0, stream.height || format.height || 0);
  if (audio) {
    native.configureAudioStream("aac", Buffer.from(audio.extraData), audio.sampleRate, audio.channels, audio.timeScale);
  }
  trace("MP4 metadata ready", { codec: stream.codec, samples: stream.samples.length, audioSamples: audio?.samples.length || 0, duration, bytes: metadata.total });
  const fullResponses = new Map();
  if (metadata.complete) fullResponses.set(format.url, metadata.data);
  if (audioMetadata?.complete) fullResponses.set(separateAudio.url, audioMetadata.data);

  async function requestRange(url, from, to, headers, signal) {
    const full = fullResponses.get(url);
    if (full) return { data: full, start: 0, complete: true };
    const range = await fetchRange(url, from, to, headers, signal);
    if (range.complete && range.start === 0 && range.data.length >= to + 1) fullResponses.set(url, range.data);
    return range;
  }

  let active = null;
  let disposed = false;
  const controller = {
    start(seconds = 0) {
      return this.seek(seconds);
    },
    pause() {
      active?.abort();
      active = null;
    },
    dispose() {
      disposed = true;
      active?.abort();
      active = null;
    },
    seek(seconds = 0, paused = false) {
      active?.abort();
      const abort = new AbortController();
      active = abort;
      const target = Math.max(0, Math.min(Number(seconds) || 0, duration || Infinity));
      if (paused) {
        native.seekVideo(target);
        return Promise.resolve();
      }
      const firstSample = syncSampleAt(stream.samples, target);
      trace("stream seek", { target, firstSample, keyframe: stream.samples[firstSample]?.timestamp });
      native.seekVideo(target);
      const task = (async () => {
        const feed = async (track, url, headers, firstSample, audioTrack = false) => {
          if (!track) return;
          let first = true;
          for (let index = firstSample; index < track.samples.length && !abort.signal.aborted && !disposed;) {
            const batch = batchEnd(track.samples, index);
            const from = track.samples[index].offset;
            const to = batch.end - 1;
            trace("fetching MP4 range", { track: audioTrack ? "audio" : "video", from, to, firstSample: index, lastSample: batch.next - 1 });
            const range = await requestRange(url, from, to, headers || {}, abort.signal);
            if (range.complete && range.data.length < to - from + 1) throw new Error("server ignored byte range for MP4 stream");
            for (let sampleIndex = index; sampleIndex < batch.next && !abort.signal.aborted && !disposed; sampleIndex += 1) {
              const sample = track.samples[sampleIndex];
              const offset = sample.offset - range.start;
              const raw = range.data.subarray(offset, offset + sample.size);
              if (raw.byteLength !== sample.size) throw new Error(`MP4 range did not contain sample ${sampleIndex}`);
              const bytes = audioTrack ? Buffer.from(raw) : sampleBytes(raw, track.nalLengthSize, track.codec);
              let accepted = false;
              while (!accepted && !abort.signal.aborted && !disposed) {
                accepted = audioTrack
                  ? native.enqueueAudioChunk(bytes, sample.timestamp, sample.duration, first, target)
                  : native.enqueueVideoChunk(bytes, sample.timestamp, sample.keyframe, first, target);
                if (!accepted) await new Promise((resolve) => setTimeout(resolve, 6));
              }
              first = false;
            }
            index = batch.next;
          }
        };
        const audioFirst = audio ? syncSampleAt(audio.samples, target) : 0;
        await Promise.all([
          feed(stream, format.url, format.http_headers, firstSample),
          audio && feed(audio, separateAudio?.url || format.url, separateAudio?.http_headers || format.http_headers, audioFirst, true),
        ]);
        if (!abort.signal.aborted && !disposed) native.finishVideoStream();
      })().catch((error) => {
        if (error?.name !== "AbortError") {
          trace("stream read failed", { error: error?.stack || String(error) });
          throw error;
        }
      }).finally(() => {
        if (active === abort) active = null;
      });
      return task;
    },
  };

  return { duration, width: stream.width || format.width || 0, height: stream.height || format.height || 0, controller };
}
