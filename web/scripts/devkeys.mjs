// Writes src/dev-keys.json from the keys of the local NCTL network in ../.node-keys, for the
// development wallet (src/devwallet.ts). The file is ignored by git. These are the well-known
// test keys of NCTL: they hold nothing outside the local network.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";

const dir = new URL("../../.node-keys/", import.meta.url);
const users = readdirSync(dir)
  .filter((name) => /^user-\d+\.pem$/.test(name))
  .sort((a, b) => a.localeCompare(b, "en", { numeric: true }))
  .map((name) => {
    const id = name.replace(".pem", "");
    const pem = readFileSync(new URL(name, dir), "utf8");
    return {
      name: id,
      publicKey: readFileSync(new URL(`${id}.pub`, dir), "utf8").trim(),
      // The PKCS#8 body of the PEM file, base64.
      pkcs8: pem.replace(/-----[A-Z ]+-----/g, "").replace(/\s+/g, ""),
    };
  });
if (users.length === 0) throw new Error("No keys in .node-keys: extract them from the NCTL container first.");
writeFileSync(new URL("../src/dev-keys.json", import.meta.url), JSON.stringify(users, null, 2) + "\n");
console.log(`Wrote ${users.length} development accounts to src/dev-keys.json`);
