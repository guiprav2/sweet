const endpoint = "https://api.x.ai/v1/responses";

function wireMessage(message) {
  if (message.type === "message") {
    return [{
      type: "message",
      role: message.role,
      content: (message.content || []).map((text) => ({
        type: message.role === "assistant" ? "output_text" : "input_text",
        text: String(text),
      })),
    }];
  }
  if (message.type === "tool_call") {
    return (message.calls || []).map((call) => ({
      type: "function_call",
      name: call.name,
      arguments: JSON.stringify(call.args || {}),
      call_id: call.call,
    }));
  }
  if (message.type === "tool_call_result") {
    return [{
      type: "function_call_output",
      call_id: message.call,
      output: typeof message.output === "string" ? message.output : JSON.stringify(message.output ?? "OK"),
    }];
  }
  return [];
}

function apiError(status, body) {
  let detail = "";
  try {
    const parsed = JSON.parse(body);
    detail = parsed.error?.message || parsed.error || parsed.message || "";
  } catch {
    detail = body.trim();
  }
  const error = new Error(detail || `xAI returned ${status}`);
  error.status = status;
  return error;
}

async function readStream(body, handlers) {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  const calls = new Map();
  while (true) {
    const { value, done } = await reader.read();
    buffer += decoder.decode(value || new Uint8Array(), { stream: !done });
    let newline;
    while ((newline = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, newline).trim();
      buffer = buffer.slice(newline + 1);
      if (!line || line === "data: [DONE]") continue;
      let event;
      try { event = JSON.parse(line.replace(/^data:\s*/, "")); } catch { continue; }
      if (event.type === "response.output_text.delta") handlers.text(event.delta || "");
      if (event.type === "response.output_item.added" && event.item?.type === "function_call") {
        calls.set(event.item.id, {
          name: event.item.name,
          call: event.item.call_id,
          arguments: event.item.arguments || "",
        });
      }
      if (event.type === "response.function_call_arguments.delta") {
        const call = calls.get(event.item_id);
        if (call) call.arguments += event.delta || "";
      }
      if (event.type === "response.function_call_arguments.done") {
        const call = calls.get(event.item_id) || {
          name: event.name,
          call: event.call_id,
          arguments: "",
        };
        call.arguments = event.arguments || call.arguments || "{}";
        handlers.call(call);
        calls.delete(event.item_id);
      }
    }
    if (done) break;
  }
}

export async function run({ logs, key, instructions, tools, signal, onText }) {
  const input = logs.flatMap(wireMessage);
  const definitions = Object.entries(tools || {}).map(([name, tool]) => ({
    type: "function",
    name,
    description: tool.description,
    parameters: tool.parameters || { type: "object", properties: {} },
  }));

  for (let turn = 0; turn < 12; turn += 1) {
    const response = await fetch(endpoint, {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: `Bearer ${key}` },
      body: JSON.stringify({
        model: "grok-4.3",
        input,
        instructions,
        tools: definitions,
        tool_choice: "auto",
        parallel_tool_calls: true,
        stream: true,
      }),
      signal,
    });
    if (!response.ok) throw apiError(response.status, await response.text());
    if (!response.body) throw new Error("xAI returned no response stream");

    let text = "";
    const pending = [];
    await readStream(response.body, {
      text: (delta) => {
        text += delta;
        onText?.(delta);
      },
      call: (call) => pending.push(call),
    });

    if (text) {
      const message = { type: "message", role: "assistant", content: [text] };
      logs.push(message);
      input.push(...wireMessage(message));
    }
    if (!pending.length) return logs;

    const calls = pending.map((call) => {
      let args = {};
      try { args = JSON.parse(call.arguments || "{}"); } catch { args = {}; }
      return { name: call.name, call: call.call, args };
    });
    const callMessage = { type: "tool_call", calls };
    logs.push(callMessage);
    input.push(...wireMessage(callMessage));
    const results = await Promise.all(calls.map(async (call) => {
      try {
        return await tools[call.name]?.handler?.(call.args) ?? { ok: false, error: `Unknown tool ${call.name}` };
      } catch (error) {
        return { ok: false, error: error.message || String(error) };
      }
    }));
    for (let index = 0; index < calls.length; index += 1) {
      const result = { type: "tool_call_result", call: calls[index].call, output: results[index] };
      logs.push(result);
      input.push(...wireMessage(result));
    }
  }
  throw new Error("DJ Grok used too many tool turns");
}

export async function verifyKey(key, signal) {
  const value = String(key || "").trim();
  if (value.length < 20) throw new Error("Enter a Grok API key.");
  const response = await fetch("https://api.x.ai/v1/models", {
    headers: { Authorization: `Bearer ${value}` },
    signal: signal || AbortSignal.timeout(15000),
  });
  if (response.status === 401 || response.status === 403) throw new Error("That Grok API key was rejected.");
  if (!response.ok) throw new Error("Could not check that key with xAI.");
  return value;
}
