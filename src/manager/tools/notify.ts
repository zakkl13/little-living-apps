// notify_user (DESIGN §9): the manager's only channel to the owner. Delivers text to Telegram
// (chunked by the client) for the chat this turn is serving.

import type { ToolModule } from "./types.js";

export type NotifyFn = (chatId: number, text: string) => Promise<void>;

export function notifyToolModule(notify: NotifyFn): ToolModule {
  return {
    specs: [
      {
        kind: "custom",
        name: "notify_user",
        description: "Send a message to the owner over Telegram. This is your reply channel.",
        input_schema: {
          type: "object",
          properties: { text: { type: "string", description: "message to send the owner" } },
          required: ["text"],
        },
      },
    ],
    handlers: {
      notify_user: async (input, ctx) => {
        const text = String(input.text ?? "").trim();
        if (!text) return { content: "nothing to send (empty text)", isError: true };
        await notify(ctx.chatId, text);
        return { content: "delivered" };
      },
    },
  };
}
