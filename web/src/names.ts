// CSPR.name: turns `name.cspr` into an account and an account into its primary name.
//
// Nothing is asked of any API. The names are read from the storage of the CSPR.name contracts,
// the way their own entry points compute them (github.com/odradev/cspr-name-contracts):
//
//   name -> account   NameToken.token.metadata[id] gives the token's resolver and expiration;
//                     DefaultResolver.nonces[id] and .resolutions[id:name:nonce] give the account.
//   account -> name   ReverseResolver.primary_names[account] gives a name, which counts only if
//                     it resolves back to the same account.
//
// The id of a name is the blake2b-256 hash of its last label, read as a big-endian number.
import { blake2b } from "@noble/hashes/blake2";
import type { NameService } from "./config";
import { ContractStorage, formattedKey, Node } from "./storage";
import nameTokenSchema from "./names/name_token_schema.json";
import resolverSchema from "./names/default_resolver_schema.json";
import reverseSchema from "./names/reverse_resolver_schema.json";

const NAME = /^([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+cspr$/;

export const looksLikeName = (text: string) => NAME.test(text.trim().toLowerCase());

export class Names {
  private readonly forward = new Map<string, Promise<string | null>>();
  private readonly reverse = new Map<string, Promise<string | null>>();

  private constructor(
    private readonly service: NameService,
    private readonly token: ContractStorage,
    private readonly resolver: ContractStorage,
    private readonly reverseResolver: ContractStorage,
  ) {}

  static async open(service: NameService): Promise<Names> {
    const node = new Node(service.rpcUrl);
    const [token, resolver, reverseResolver] = await Promise.all([
      ContractStorage.open(node, nameTokenSchema, service.nameToken),
      ContractStorage.open(node, resolverSchema, service.resolver),
      ContractStorage.open(node, reverseSchema, service.reverseResolver),
    ]);
    return new Names(service, token, resolver, reverseResolver);
  }

  /** Said next to a resolved name when the names are not those of the network of the deals. */
  get note(): string | undefined {
    return this.service.note;
  }

  /** The account (`account-hash-…`) a name points at, or `null` if it points at nothing. */
  resolve(name: string): Promise<string | null> {
    const wanted = name.trim().toLowerCase();
    let found = this.forward.get(wanted);
    if (!found) {
      found = this.readForward(wanted).catch((error) => {
        this.forward.delete(wanted);
        throw error;
      });
      this.forward.set(wanted, found);
    }
    return found;
  }

  /** The primary name of an account, or `null` if it has none. */
  nameOf(account: string): Promise<string | null> {
    let found = this.reverse.get(account);
    if (!found) {
      found = this.readReverse(account).catch(() => {
        this.reverse.delete(account);
        return null;
      });
      this.reverse.set(account, found);
    }
    return found;
  }

  private async readForward(name: string): Promise<string | null> {
    if (!NAME.test(name)) return null;
    const label = name.slice(0, -".cspr".length).split(".").pop()!;
    const id = BigInt(`0x${Array.from(blake2b(label, { dkLen: 32 }), (b) => b.toString(16).padStart(2, "0")).join("")}`).toString();

    const metadata = await this.token.read("token.metadata", [id]);
    if (metadata === null) return null;
    // Printed as `key:value, key:value`. The resolver of the token must be the one read below.
    const expiration = /(?:^|, )expiration:(\d+)/.exec(metadata)?.[1];
    const resolver = /(?:^|, )resolver:(hash-[0-9a-f]{64})/.exec(metadata)?.[1];
    if (!expiration || resolver !== this.service.resolver) return null;
    // The stored expirations are far larger than a time in milliseconds; they read as
    // microseconds. A name past either reading of its expiration is treated as expired.
    const stored = BigInt(expiration);
    const expiresAt = stored > 100_000_000_000_000n ? stored / 1000n : stored;
    if (expiresAt < BigInt(Date.now())) return null;

    const nonce = (await this.resolver.read("nonces", [id])) ?? "0";
    const resolution = await this.resolver.read("resolutions", [`${id}:${name}:${nonce}`]);
    if (resolution === null || !/Key::Account\(/.test(resolution)) return null;
    return formattedKey(resolution);
  }

  private async readReverse(account: string): Promise<string | null> {
    const stored = await this.reverseResolver.read("primary_names", [account]);
    const name = stored === null ? null : /[a-z0-9.-]+\.cspr/.exec(stored)?.[0];
    if (!name) return null;
    return (await this.resolve(name)) === account ? name : null;
  }
}
