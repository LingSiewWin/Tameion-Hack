// Names and the lender link (docs/105). The checksum vectors are ERC-7828's own examples
// (https://eips.ethereum.org/EIPS/eip-7828); the join digest is the one crates/operator/src/desk_invite.rs
// asserts (`the_join_digest_is_the_one_the_page_signs`).
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { hashTypedData } from "viem";
import { chainLabel, cleanName, codeText, deskJoin, interopChecksum, interopName, nameAt } from "../src/lib/names.ts";
import { kitFromFragment, lenderLink } from "../src/lib/links.ts";

test("ERC-7828 checksums match the spec's examples", () => {
  assert.equal(interopChecksum("0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045", 1), "4CA88C9C");
  assert.equal(interopChecksum("0xFe89cc7aBB2C4183683ab71653C4cdc9B02D44b7", 1), "80B12379");
  assert.equal(interopName("0xfe89cc7abb2c4183683ab71653c4cdc9b02d44b7", 1), "0xFe89cc7aBB2C4183683ab71653C4cdc9B02D44b7@eip155:1#80B12379");
  // A chain id over 255 takes more bytes (CAIP-350: Sepolia is 0xaa36a7), so the same address has another code.
  assert.notEqual(interopChecksum("0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045", 421614), "4CA88C9C");
  assert.equal(codeText("0xFe89cc7aBB2C4183683ab71653C4cdc9B02D44b7", 1), "80B1 2379");
  assert.equal(nameAt("Supplier", 421614), "Supplier@arbitrum-sepolia");
  assert.equal(chainLabel(999), "eip155:999");
});

test("the join the page signs is the digest the service checks", () => {
  const t = deskJoin(421614, "0x" + "d5".repeat(20), "Kim Seng Bullion", "Supplier", "ab12", 1_792_100_000);
  assert.equal(hashTypedData(t), "0x1dd441fc93b8b4e0f674e5357cf5f95594b246acc50d8e8701fcb58bd3a3dc4f");
});

test("a statement travels in the link byte for byte, and a changed link is refused", async () => {
  const kit = readFileSync(new URL("../../../proof/desk-console-kit-GOLD-W8B-1792079650-statement-1792079680.json", import.meta.url), "utf8");
  const link = await lenderLink(kit, "https://desk.example/", "desk-gold-2329");
  assert.ok(link.startsWith("https://desk.example/lender#s="));
  const fragment = link.slice(link.indexOf("#"));
  // The 5,348-byte kit of docs/102 D14 travels as about 3 KB of link.
  assert.ok(fragment.length < 3_100, `fragment is ${fragment.length} characters`);
  const back = await kitFromFragment(fragment);
  assert.equal(back?.text, kit);
  assert.equal(back?.from, "desk-gold-2329");
  assert.equal(await kitFromFragment("#from=x"), null);
  const s = new URLSearchParams(fragment.slice(1)).get("s")!;
  await assert.rejects(kitFromFragment(`#s=${s.slice(0, s.length - 40)}`), /cut or changed/);
});

test("a name is cleaned as the service cleans it (docs/103 S2-2)", () => {
  // The reviewer's look-alike: an invisible U+200B does not survive, so it cannot be a second name.
  assert.equal(cleanName("Lim Brothers Gold\u200B"), "Lim Brothers Gold");
  assert.equal(cleanName("\u202ELim Brothers\u200D Gold"), "Lim Brothers Gold");
  // The same outputs the Rust test asserts for `clean_name`.
  assert.equal(cleanName("\uFF2Cim  Brothers"), "Lim Brothers");
  assert.equal(cleanName("金城珠宝"), "金城珠宝");
  assert.equal(cleanName("\u200B\u200B"), "");
  // Every White_Space character Rust's split_whitespace splits on, and only those.
  assert.equal(cleanName("a\u0085b\u3000c\u00a0d"), "a b c d");
});
