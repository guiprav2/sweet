import { run } from './grok.js';
import { createTools, describeTool } from './tools.js';

let INSTRUCTIONS = `You are DJ Grok, the DJ inside this player. Speak in short plain sentences. No markdown, no bullet lists, no stage directions.

You stream YouTube videos and keep thematic playlists. The right-hand sidebar shows those playlists. Clicking a name there starts its first song. Your play_playlist tool does the same thing and can also start later in the list.

Rules:
- Never claim a song or playlist is playing unless a tool result from this turn says ok and names the track. Describing the snapshot is not playback. Call play_playlist or play_song before you say anything is on.
- A request for one song or artist uses play_song. That is a one-off and does not join a saved playlist.
- A request for a playlist, a vibe, or a mood uses the saved library. Read the library snapshot. Call list_playlists if you need it. Then call play_playlist with the playlist name.
- "Play a more soothing playlist" means calmer, quieter, slower, or more ambient than the playlist that is on now. Pick the saved playlist whose mood best matches. If the current one is already the calmest, say so and offer to build a quieter one. Do not restart the same playlist unless they ask.
- The same comparison applies to darker, brighter, more energetic, sadder, and similar relative requests.
- If nothing saved fits, create_playlist with a clear mood, a theme, several real song searches, and play true.
- play_playlist starts at the first song when start_at is omitted. Pass start_at only when they name a later track or a position. A number is 1-based. A title fragment matches inside that playlist.
- After a tool plays something, mention the playlist and the actual track title from the tool result.
- Do not invent songs, video ids, or playlist names.`;

export async function runTurn({ library, youtube, text, signal, onEvent }) {
  let key = await library.getApiKey();
  if (!key) {
    let error = new Error('Add your Grok API key to start DJ Grok.');
    error.status = 400;
    throw error;
  }
  let prompt = String(text || '').trim();
  if (!prompt) {
    let error = new Error('Say something to DJ Grok.');
    error.status = 400;
    throw error;
  }
  if (prompt.length > 4000) {
    let error = new Error('That message is too long.');
    error.status = 400;
    throw error;
  }

  await library.addMessage('user', prompt);
  onEvent?.('state', await library.publicState());

  let logs = await library.loadLogs();
  logs.push({ type: 'message', role: 'user', content: [prompt] });
  let assistant = '';
  let played = false;
  let tools = createTools(library, youtube);
  let wrapped = {};
  for (let [name, spec] of Object.entries(tools)) {
    wrapped[name] = {
      description: spec.description,
      parameters: spec.parameters,
      handler: async (args) => {
        onEvent?.('tool', { name, phase: 'start' });
        let result = await spec.handler(args || {});
        if (result?.ok && (name === 'play_playlist' || name === 'play_song' || name === 'transport' || name === 'create_playlist')) {
          played = true;
        }
        let summary = describeTool(name, result);
        await library.addMessage('tool', summary);
        onEvent?.('tool', { name, summary, ok: !!result?.ok });
        onEvent?.('state', await library.publicState());
        return result;
      },
    };
  }

  let askPlay = /\b(play|put on|start)\b/i.test(prompt) && !/\b(what|whats|what's)\b/i.test(prompt);

  async function complete() {
    let snapshot = await library.snapshotText();
    await run({
      logs,
      key,
      signal,
      instructions: INSTRUCTIONS + '\n\n' + snapshot,
      tools: wrapped,
      onText: (chunk) => {
        if (chunk) {
          assistant += chunk;
          onEvent?.('delta', { text: chunk });
        }
      },
    });
  }

  try {
    await complete();
    if (!played && askPlay) {
      onEvent?.('reset', {});
      logs.push({
        type: 'message',
        role: 'user',
        content: ['You described playback without calling a tool. Call play_playlist or play_song now. Mention only the track that tool returns.'],
      });
      assistant = '';
      await complete();
    }
  } finally {
    if (assistant.trim()) await library.addMessage('assistant', assistant.trim());
    await library.saveLogs(logs);
  }
  return assistant.trim();
}
