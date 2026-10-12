import { snapshotText } from './library.js';

function clip(text, max = 180) {
  return String(text || '').replace(/\s+/g, ' ').trim().slice(0, max);
}

async function pool(items, limit, fn) {
  let results = new Array(items.length);
  let cursor = 0;
  async function worker() {
    while (cursor < items.length) {
      let index = cursor;
      cursor += 1;
      results[index] = await fn(items[index], index);
    }
  }
  let width = Math.min(limit, items.length);
  if (width > 0) await Promise.all(Array.from({ length: width }, () => worker()));
  return results;
}

async function searchOne(youtube, query) {
  let q = clip(query);
  if (!q) return { query: q, hit: null, error: 'empty search' };
  try {
    let hits = await youtube.search(q, 1);
    return { query: q, hit: hits[0] || null, error: hits[0] ? '' : 'no YouTube result' };
  } catch (error) {
    return { query: q, hit: null, error: error.message || String(error) };
  }
}

function songList(value) {
  let items = Array.isArray(value) ? value : typeof value === 'string' ? [value] : [];
  let queries = [];
  for (let item of items) {
    let text = clip(item);
    if (text && !queries.includes(text)) queries.push(text);
    if (queries.length >= 12) break;
  }
  return queries;
}

function pubTrack(track) {
  if (!track) return null;
  return {
    title: track.title,
    channel: track.channel,
    videoId: track.videoId,
    duration: track.duration,
  };
}

export function describeTool(name, result) {
  if (!result?.ok) return `${name} failed: ${result?.error || 'unknown error'}`;
  if (name === 'play_song') return `Playing “${result.track.title}” as a single`;
  if (name === 'play_playlist') {
    return `Playing “${result.playlist}” from track ${result.index}, “${result.track.title}”`;
  }
  if (name === 'create_playlist') {
    let started = result.started?.ok ? `, started at track ${result.started.index}` : '';
    return `Created “${result.playlist.name}” with ${result.playlist.songs.length} songs${started}`;
  }
  if (name === 'add_to_playlist') return `Added ${result.added.length} song${result.added.length === 1 ? '' : 's'} to “${result.playlist}”`;
  if (name === 'remove_from_playlist') return `Removed “${result.removed}” from “${result.playlist}”`;
  if (name === 'update_playlist') return `Updated “${result.playlist.name}”`;
  if (name === 'delete_playlist') return `Deleted “${result.deleted}”`;
  if (name === 'transport') {
    if (result.track) return `Player ${result.status}${result.index ? ` · track ${result.index}` : ''}: “${result.track.title}”`;
    return `Player ${result.status}`;
  }
  if (name === 'search_youtube') return `Searched YouTube · ${result.results.length} hit${result.results.length === 1 ? '' : 's'}`;
  if (name === 'list_playlists') return `Listed ${result.playlists.length} playlist${result.playlists.length === 1 ? '' : 's'}`;
  if (name === 'now_playing') return result.line || 'Now playing';
  return `${name} ok`;
}

export function createTools(library, youtube) {
  async function guard(fn) {
    try { return await fn(); } catch (error) { return { ok: false, error: error.message || String(error) }; }
  }

  return {
    search_youtube: {
      description: 'Search YouTube for songs. Returns titles, channels, durations, and video ids. Does not play or save anything.',
      parameters: {
        type: 'object',
        properties: {
          query: { type: 'string', description: 'Song, artist, or description to search on YouTube' },
          limit: { type: 'integer', description: 'How many results, 1 to 8. Default 5.' },
        },
        required: ['query'],
      },
      handler: (args) => guard(async () => {
        let hits = await youtube.search(args?.query, args?.limit || 5);
        return {
          ok: true,
          results: hits.map((hit) => pubTrack(hit)),
        };
      }),
    },

    play_song: {
      description: 'Search YouTube and immediately stream one full video as a one-off. It does not join a saved playlist. Use this when the user names a song or artist rather than a playlist.',
      parameters: {
        type: 'object',
        properties: {
          query: { type: 'string', description: 'Song and artist to search on YouTube' },
          pick: { type: 'integer', description: '1-based result to play. Omit to play the best match.' },
        },
        required: ['query'],
      },
      handler: (args) => guard(async () => {
        let hits = await youtube.search(args?.query, 5);
        if (!hits.length) return { ok: false, error: 'No YouTube results for that song' };
        let pick = Math.floor(Number(args?.pick) || 1);
        if (pick < 1 || pick > hits.length) pick = 1;
        let played = await library.playOneOff({ ...hits[pick - 1], query: clip(args.query) });
        return played.ok ? { ...played, pick } : played;
      }),
    },

    list_playlists: {
      description: 'List saved thematic playlists with mood, theme, and song titles. Use this before choosing a playlist by mood, including when the user wants something more soothing, darker, or more energetic than what is on.',
      parameters: { type: 'object', properties: {} },
      handler: () => guard(async () => {
        let state = await library.publicState();
        return {
          ok: true,
          now: state.player.track ? {
            title: state.player.track.title,
            status: state.player.status,
            playlist: state.player.playlistName || null,
            index: state.player.index + 1,
          } : null,
          playlists: state.playlists.map((playlist) => ({
            name: playlist.name,
            mood: playlist.mood,
            theme: playlist.theme,
            songs: playlist.tracks.map((track, index) => `${index + 1}. ${track.title}`),
          })),
        };
      }),
    },

    create_playlist: {
      description: 'Create a thematic playlist. Each entry in songs is a YouTube search for one track; the top result is saved. Set mood to a short comparable phrase such as "soothing ambient" or "tense electronic". Pass play true to start it. Playback starts at the first song unless start_at is set.',
      parameters: {
        type: 'object',
        properties: {
          name: { type: 'string', description: 'Playlist name shown in the sidebar' },
          mood: { type: 'string', description: 'Short mood label used to compare playlists, for example "soothing ambient"' },
          theme: { type: 'string', description: 'One sentence about the theme' },
          songs: {
            type: 'array',
            items: { type: 'string' },
            description: 'YouTube search queries, one per song, up to 12. Example: "Satie Gymnopedie No.1"',
          },
          play: { type: 'boolean', description: 'Start playback after saving. Default false.' },
          start_at: {
            type: 'string',
            description: 'Used only when play is true. Omit or "1" for the first song. A positive integer is the 1-based track. Any other string matches a title.',
          },
        },
        required: ['name', 'mood', 'songs'],
      },
      handler: (args) => guard(async () => {
        let queries = songList(args?.songs);
        if (!queries.length) return { ok: false, error: 'Provide at least one song search' };
        let found = await pool(queries, 3, (query) => searchOne(youtube, query));
        let tracks = [];
        let missed = [];
        let seen = new Set();
        for (let item of found) {
          if (!item.hit) {
            missed.push({ query: item.query, error: item.error });
            continue;
          }
          if (seen.has(item.hit.videoId)) continue;
          seen.add(item.hit.videoId);
          tracks.push({ ...item.hit, query: item.query });
        }
        if (!tracks.length) return { ok: false, error: 'No YouTube results for those songs', missed };
        let created = await library.createPlaylist({
          name: args?.name,
          theme: args?.theme || '',
          mood: args?.mood || '',
          tracks,
        });
        if (!created.ok) return created;
        let started = null;
        if (args?.play === true) started = await library.playPlaylist(created.playlist.id, args?.start_at);
        return {
          ok: true,
          playlist: {
            name: created.playlist.name,
            mood: created.playlist.mood,
            theme: created.playlist.theme,
            songs: created.playlist.tracks.map((track) => track.title),
          },
          missed,
          started,
        };
      }),
    },

    add_to_playlist: {
      description: 'Search YouTube and append songs to an existing playlist.',
      parameters: {
        type: 'object',
        properties: {
          playlist: { type: 'string', description: 'Playlist name or id' },
          songs: { type: 'array', items: { type: 'string' }, description: 'YouTube search queries to append' },
        },
        required: ['playlist', 'songs'],
      },
      handler: (args) => guard(async () => {
        let queries = songList(args?.songs);
        if (!queries.length) return { ok: false, error: 'Provide at least one song search' };
        let found = await pool(queries, 3, (query) => searchOne(youtube, query));
        let tracks = [];
        let missed = [];
        for (let item of found) {
          if (!item.hit) missed.push({ query: item.query, error: item.error });
          else tracks.push({ ...item.hit, query: item.query });
        }
        if (!tracks.length) return { ok: false, error: 'No YouTube results for those songs', missed };
        let added = await library.addTracks(args.playlist, tracks);
        return added.ok ? { ...added, missed } : { ...added, missed };
      }),
    },

    remove_from_playlist: {
      description: 'Remove one song from a playlist by 1-based position or by part of its title.',
      parameters: {
        type: 'object',
        properties: {
          playlist: { type: 'string', description: 'Playlist name or id' },
          song: { type: 'string', description: '1-based track number, or part of the song title' },
        },
        required: ['playlist', 'song'],
      },
      handler: (args) => guard(() => library.removeFromPlaylist(args?.playlist, args?.song)),
    },

    update_playlist: {
      description: 'Rename a playlist or change its mood and theme. Mood is how later requests such as "play a more soothing playlist" tell playlists apart.',
      parameters: {
        type: 'object',
        properties: {
          playlist: { type: 'string', description: 'Current playlist name or id' },
          name: { type: 'string' },
          mood: { type: 'string' },
          theme: { type: 'string' },
        },
        required: ['playlist'],
      },
      handler: (args) => guard(() => library.updatePlaylist(args?.playlist, {
        name: args?.name,
        mood: args?.mood,
        theme: args?.theme,
      })),
    },

    delete_playlist: {
      description: 'Delete a saved playlist and its songs. If it is playing, playback stops.',
      parameters: {
        type: 'object',
        properties: {
          playlist: { type: 'string', description: 'Playlist name or id' },
        },
        required: ['playlist'],
      },
      handler: (args) => guard(() => library.deletePlaylist(args?.playlist)),
    },

    play_playlist: {
      description: 'Start a saved playlist and stream it now. Omit start_at to start at the first song. This is the tool for "play this playlist", for clicking-equivalent playback, and for mood requests such as "play a more soothing playlist": compare moods in the library snapshot, pick the playlist that fits, and call this. If the user names a later song or a track number, pass that as start_at.',
      parameters: {
        type: 'object',
        properties: {
          playlist: { type: 'string', description: 'Playlist name, id, or a unique mood/theme phrase' },
          start_at: {
            type: 'string',
            description: 'Optional. Omit or "1" to start at the first song. A positive integer is the 1-based track number. Any other string matches a title inside the playlist.',
          },
        },
        required: ['playlist'],
      },
      handler: (args) => guard(() => library.playPlaylist(args?.playlist, args?.start_at)),
    },

    transport: {
      description: 'Control the player that is already loaded: pause, resume, next, previous, or stop.',
      parameters: {
        type: 'object',
        properties: {
          action: { type: 'string', enum: ['pause', 'resume', 'next', 'previous', 'stop'] },
        },
        required: ['action'],
      },
      handler: (args) => guard(() => library.transport(args?.action)),
    },

    now_playing: {
      description: 'Read what is loaded, including playlist mood and the current track index.',
      parameters: { type: 'object', properties: {} },
      handler: () => guard(async () => {
        let state = await library.publicState();
        let line = snapshotText(state).split('\n').slice(1, 3).join(' ');
        return {
          ok: true,
          line,
          status: state.player.status,
          playlist: state.player.playlistName || null,
          index: state.player.track ? state.player.index + 1 : 0,
          track: pubTrack(state.player.track),
        };
      }),
    },
  };
}
