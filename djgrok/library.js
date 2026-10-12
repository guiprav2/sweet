import crypto from "node:crypto";
import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import { dirname } from "node:path";

export function resolveStart(tracks, startAt) {
  if (startAt == null || String(startAt).trim() === "") return { index: 0 };
  const raw = String(startAt).trim();
  if (/^\d+$/.test(raw)) {
    const number = Number(raw);
    if (number < 1 || number > tracks.length) return { error: `No track ${number}. This playlist has ${tracks.length} songs.` };
    return { index: number - 1 };
  }
  const index = tracks.findIndex((track) => track.title.toLowerCase().includes(raw.toLowerCase()));
  return index < 0 ? { error: `No song matching “${raw}”.` } : { index };
}

export function findPlaylist(playlists, nameOrId) {
  const key = String(nameOrId || "").trim();
  if (!key) return { error: "Which playlist?" };
  const exact = playlists.filter((item) => item.id === key || item.name.toLowerCase() === key.toLowerCase());
  if (exact.length === 1) return { playlist: exact[0] };
  const lower = key.toLowerCase();
  for (const read of [(p) => p.name, (p) => p.mood, (p) => p.theme]) {
    const found = playlists.filter((item) => String(read(item) || "").toLowerCase().includes(lower));
    if (found.length === 1) return { playlist: found[0] };
    if (found.length > 1) return { error: "Several playlists match. Pick one by its exact name." };
  }
  return { error: `No playlist matches “${key}”.` };
}

export function snapshotText(state) {
  const lines = ["Library snapshot. This is the source of truth for what is on."];
  const player = state.player;
  if (!player.track) lines.push("Nothing is playing.");
  else {
    const where = player.playlistName
      ? `playlist “${player.playlistName}”, song ${player.index + 1} of ${player.tracks.length}`
      : "a one-off song, not saved to a playlist";
    const playlist = state.playlists.find((item) => item.id === player.playlistId);
    lines.push(`Now: “${player.track.title}” by ${player.track.channel || "unknown"} (${player.status}), ${where}${playlist?.mood ? `, mood: ${playlist.mood}` : ""}.`);
  }
  if (!state.playlists.length) lines.push("Saved playlists: none.");
  else {
    lines.push("Saved playlists, oldest first:");
    for (const playlist of state.playlists) {
      lines.push(`- ${playlist.name} | mood: ${playlist.mood || "unset"} | theme: ${playlist.theme || "unset"} | ${playlist.tracks.length} songs`);
      playlist.tracks.slice(0, 24).forEach((track, index) => lines.push(`  ${index + 1}. ${track.title}${track.channel ? ` — ${track.channel}` : ""}`));
    }
  }
  return lines.join("\n");
}

function cleanTrack(track, playlistId = null, position = 0) {
  if (!/^[\w-]{11}$/.test(track?.videoId || "")) throw new Error("bad video id");
  return {
    id: track.id || crypto.randomUUID(),
    playlistId,
    position,
    videoId: track.videoId,
    title: String(track.title || "Untitled").slice(0, 300),
    channel: String(track.channel || "").slice(0, 200),
    duration: Number(track.duration) || 0,
    thumbnail: track.thumbnail || `https://i.ytimg.com/vi/${track.videoId}/hqdefault.jpg`,
    query: String(track.query || "").slice(0, 180),
  };
}

export async function createLibrary(file, seedPlaylists = []) {
  let stored;
  try { stored = JSON.parse(await readFile(file, "utf8")); } catch { stored = null; }
  const data = stored && Array.isArray(stored.playlists) ? stored : {
    playlists: seedPlaylists.map((playlist) => ({
      ...playlist,
      theme: playlist.theme || "",
      tracks: playlist.tracks.map((track, index) => cleanTrack(track, playlist.id, index)),
    })),
    player: { playlistId: null, index: 0, status: "idle", oneOff: null },
    messages: [],
    logs: [],
    apiKey: "",
    nextMessage: 1,
  };
  data.messages ||= [];
  data.logs ||= [];
  data.nextMessage ||= data.messages.length + 1;
  let onChange = null;
  let chain = Promise.resolve();

  async function save() {
    await mkdir(dirname(file), { recursive: true });
    const temporary = `${file}.tmp`;
    await writeFile(temporary, JSON.stringify(data, null, 2), { mode: 0o600 });
    await rename(temporary, file);
  }

  function locked(fn) {
    const pending = chain.then(fn, fn);
    chain = pending.then(() => {}, () => {});
    return pending;
  }

  function derived() {
    const playlists = data.playlists;
    let tracks = [];
    let playlistName = "";
    let oneOff = false;
    if (data.player.playlistId) {
      const playlist = playlists.find((item) => item.id === data.player.playlistId);
      tracks = playlist?.tracks || [];
      playlistName = playlist?.name || "";
    } else if (data.player.oneOff) {
      tracks = [data.player.oneOff];
      oneOff = true;
    }
    const index = tracks.length ? Math.max(0, Math.min(data.player.index || 0, tracks.length - 1)) : 0;
    return structuredClone({
      playlists,
      messages: data.messages,
      player: {
        status: tracks.length ? data.player.status : "idle",
        playlistId: data.player.playlistId || null,
        playlistName,
        oneOff,
        index,
        track: tracks[index] || null,
        tracks,
      },
    });
  }

  async function changed() {
    await save();
    if (onChange) await onChange(derived());
  }

  async function publicState() { return derived(); }

  async function createPlaylist({ name, theme = "", mood = "", tracks }) {
    return locked(async () => {
      const cleanName = String(name || "").trim().slice(0, 80);
      if (!cleanName) return { ok: false, error: "Playlist name required" };
      if (!tracks?.length) return { ok: false, error: "A playlist needs at least one song" };
      const id = crypto.randomUUID();
      const playlist = {
        id,
        name: cleanName,
        theme: String(theme || "").slice(0, 240),
        mood: String(mood || "").slice(0, 80),
        tracks: tracks.map((track, index) => cleanTrack(track, id, index)),
      };
      data.playlists.push(playlist);
      await changed();
      return { ok: true, playlist: structuredClone(playlist) };
    });
  }

  async function addTracks(nameOrId, tracks) {
    return locked(async () => {
      const found = findPlaylist(data.playlists, nameOrId);
      if (found.error) return { ok: false, error: found.error };
      const existing = new Set(found.playlist.tracks.map((track) => track.videoId));
      const fresh = tracks.filter((track) => track?.videoId && !existing.has(track.videoId));
      if (!fresh.length) return { ok: false, error: "Those songs are already on the playlist" };
      const added = fresh.map((track, offset) => cleanTrack(track, found.playlist.id, found.playlist.tracks.length + offset));
      found.playlist.tracks.push(...added);
      await changed();
      return { ok: true, playlist: found.playlist.name, added: added.map((track) => track.title) };
    });
  }

  async function removeFromPlaylist(nameOrId, song) {
    return locked(async () => {
      const found = findPlaylist(data.playlists, nameOrId);
      if (found.error) return { ok: false, error: found.error };
      const start = resolveStart(found.playlist.tracks, song);
      if (start.error) return { ok: false, error: start.error };
      const [removed] = found.playlist.tracks.splice(start.index, 1);
      found.playlist.tracks.forEach((track, index) => { track.position = index; });
      if (data.player.playlistId === found.playlist.id) {
        if (!found.playlist.tracks.length) data.player = { playlistId: null, index: 0, status: "idle", oneOff: null };
        else data.player.index = Math.min(data.player.index, found.playlist.tracks.length - 1);
      }
      await changed();
      return { ok: true, removed: removed.title, playlist: found.playlist.name };
    });
  }

  async function updatePlaylist(nameOrId, fields = {}) {
    return locked(async () => {
      const found = findPlaylist(data.playlists, nameOrId);
      if (found.error) return { ok: false, error: found.error };
      if (typeof fields.name === "string" && fields.name.trim()) found.playlist.name = fields.name.trim().slice(0, 80);
      if (typeof fields.theme === "string") found.playlist.theme = fields.theme.slice(0, 240);
      if (typeof fields.mood === "string") found.playlist.mood = fields.mood.slice(0, 80);
      await changed();
      return { ok: true, playlist: structuredClone(found.playlist) };
    });
  }

  async function deletePlaylist(nameOrId) {
    return locked(async () => {
      const found = findPlaylist(data.playlists, nameOrId);
      if (found.error) return { ok: false, error: found.error };
      data.playlists = data.playlists.filter((item) => item.id !== found.playlist.id);
      if (data.player.playlistId === found.playlist.id) data.player = { playlistId: null, index: 0, status: "idle", oneOff: null };
      await changed();
      return { ok: true, deleted: found.playlist.name };
    });
  }

  async function playPlaylist(nameOrId, startAt) {
    return locked(async () => {
      const found = findPlaylist(data.playlists, nameOrId);
      if (found.error) return { ok: false, error: found.error };
      if (!found.playlist.tracks.length) return { ok: false, error: "That playlist has no songs" };
      const start = resolveStart(found.playlist.tracks, startAt);
      if (start.error) return { ok: false, error: start.error };
      data.player = { playlistId: found.playlist.id, index: start.index, status: "playing", oneOff: null };
      await changed();
      return {
        ok: true,
        playlist: found.playlist.name,
        mood: found.playlist.mood,
        theme: found.playlist.theme,
        index: start.index + 1,
        count: found.playlist.tracks.length,
        track: structuredClone(found.playlist.tracks[start.index]),
      };
    });
  }

  async function playOneOff(track) {
    return locked(async () => {
      const stored = cleanTrack({ ...track, id: "one-off" });
      data.player = { playlistId: null, index: 0, status: "playing", oneOff: stored };
      await changed();
      return { ok: true, track: structuredClone(stored) };
    });
  }

  async function transport(action) {
    return locked(async () => {
      const state = derived();
      if (action === "stop") {
        data.player = { playlistId: null, index: 0, status: "idle", oneOff: null };
        await changed();
        return { ok: true, status: "idle" };
      }
      if (!state.player.track) return { ok: false, error: "Nothing is loaded" };
      if (action === "pause" || action === "resume") data.player.status = action === "pause" ? "paused" : "playing";
      else if (action === "next" || action === "previous") {
        const delta = action === "next" ? 1 : -1;
        data.player.index = (state.player.index + delta + state.player.tracks.length) % state.player.tracks.length;
        data.player.status = "playing";
      } else return { ok: false, error: "Unknown transport action" };
      await changed();
      const next = derived().player;
      return { ok: true, status: next.status, index: next.index + 1, track: next.track };
    });
  }

  async function addMessage(role, body) {
    return locked(async () => {
      const text = String(body || "").slice(0, 8000);
      if (!text.trim()) return null;
      const id = data.nextMessage++;
      data.messages.push({ id, role, body: text });
      if (data.messages.length > 100) data.messages = data.messages.slice(-100);
      await changed();
      return id;
    });
  }

  return {
    publicState,
    snapshotText: async () => snapshotText(derived()),
    createPlaylist,
    addTracks,
    removeFromPlaylist,
    updatePlaylist,
    deletePlaylist,
    playPlaylist,
    playOneOff,
    transport,
    addMessage,
    getApiKey: async () => String(data.apiKey || ""),
    setApiKey: (key) => locked(async () => { data.apiKey = String(key || "").trim(); await save(); }),
    clearApiKey: () => locked(async () => { data.apiKey = ""; data.logs = []; await save(); }),
    loadLogs: async () => structuredClone(data.logs || []),
    saveLogs: (logs) => locked(async () => { data.logs = structuredClone(logs.slice(-40)); await save(); }),
    set onChange(fn) { onChange = fn; },
    get onChange() { return onChange; },
  };
}
