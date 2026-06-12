import { strict as assert } from "node:assert";
import { describe, it } from "node:test";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { chunkText, type OutboundPhoto, type TelegramClient } from "../src/transport/telegram.js";
import { createTelegramDeliver } from "../src/transport/deliver.js";
import { extractAttachments } from "../src/manager/driver.js";

describe("chunkText", () => {
  it("returns a single chunk when under the limit", () => {
    assert.deepEqual(chunkText("hello"), ["hello"]);
    assert.deepEqual(chunkText(""), []);
  });

  it("splits long text into <=4096 pieces with no loss", () => {
    const text = "X".repeat(9000);
    const chunks = chunkText(text);
    assert.equal(chunks.length, 3);
    assert.ok(chunks.every((c) => c.length <= 4096));
    assert.equal(chunks.join("").length, 9000);
  });

  it("prefers to break on newline boundaries", () => {
    const block = "a".repeat(4000) + "\n" + "b".repeat(4000);
    const chunks = chunkText(block);
    assert.equal(chunks.length, 2);
    assert.equal(chunks[0], "a".repeat(4000));
    assert.equal(chunks[1], "b".repeat(4000));
  });
});

describe("extractAttachments", () => {
  it("pulls ATTACH lines out and keeps the rest of the text", () => {
    const { text, attachments } = extractAttachments(
      "Done!\nATTACH: /tmp/lila-shots/a.png\nDetails below.\n  ATTACH: /tmp/lila-shots/b.png  ",
    );
    assert.equal(text, "Done!\nDetails below.");
    assert.deepEqual(attachments, ["/tmp/lila-shots/a.png", "/tmp/lila-shots/b.png"]);
  });

  it("ignores ATTACH mentioned mid-line or with a relative path", () => {
    const reply = "I used ATTACH: /x.png syntax earlier.\nATTACH: relative.png";
    const { text, attachments } = extractAttachments(reply);
    assert.equal(text, reply);
    assert.deepEqual(attachments, []);
  });

  it("returns plain replies untouched", () => {
    assert.deepEqual(extractAttachments("just text"), { text: "just text", attachments: [] });
  });
});

describe("createTelegramDeliver", () => {
  // A recording stub of the two client methods deliver uses; the rest are never called.
  function stubClient() {
    const messages: Array<{ chatId: number; text: string }> = [];
    const photos: Array<{ chatId: number; photo: OutboundPhoto }> = [];
    const client = {
      sendMessage: async (chatId: number, text: string) => {
        messages.push({ chatId, text });
        return 1;
      },
      sendPhoto: async (chatId: number, photo: OutboundPhoto) => {
        photos.push({ chatId, photo });
        return 2;
      },
    } as unknown as TelegramClient;
    return { client, messages, photos };
  }

  it("sends the text, then each existing image attachment as a photo", async () => {
    const dir = mkdtempSync(join(tmpdir(), "lila-shots-"));
    const shot = join(dir, "home.png");
    writeFileSync(shot, Buffer.from("fake png bytes"));

    const { client, messages, photos } = stubClient();
    await createTelegramDeliver(client)(7, "Here's the new home page.", [shot]);

    assert.deepEqual(messages, [{ chatId: 7, text: "Here's the new home page." }]);
    assert.equal(photos.length, 1);
    assert.equal(photos[0]!.photo.filename, "home.png");
    assert.equal(photos[0]!.photo.bytes.toString(), "fake png bytes");
  });

  it("drops a missing or non-image attachment with a visible note, keeping the text", async () => {
    const { client, messages, photos } = stubClient();
    await createTelegramDeliver(client)(7, "Done.", ["/nope/gone.png", "/etc/passwd"]);

    assert.equal(photos.length, 0);
    assert.equal(messages.length, 1);
    assert.match(messages[0]!.text, /^Done\./);
    assert.match(messages[0]!.text, /couldn't attach gone\.png/);
    assert.match(messages[0]!.text, /couldn't attach passwd/);
  });

  it("sends nothing for an empty text with no attachments", async () => {
    const { client, messages, photos } = stubClient();
    await createTelegramDeliver(client)(7, "", []);
    assert.equal(messages.length, 0);
    assert.equal(photos.length, 0);
  });
});
