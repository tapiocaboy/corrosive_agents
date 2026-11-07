# solana_agents — decentralised, multi-chain on-chain agents

This use case shows how a [`corrosive_agents`](../../README.md) agent integrates
with **blockchains** — reading **Solana and Solana-like (SVM)** networks (Solana
testnet / devnet / mainnet, [Eclipse](https://www.eclipse.xyz),
[Sonic](https://www.sonic.game), …) and **EVM** chains over JSON-RPC — and how
to wire one agent **per network** into a **decentralised, peer-to-peer system**
over [libp2p](https://libp2p.io) with no central broker.

It runs two ways:

| Mode | What it is | Command |
|------|------------|---------|
| **`solo`** | One agent owns SVM + EVM tools; the LLM decides which to call | `cargo run -- solo "…"` |
| **mesh** | A `client` asks an `orchestrator`, which fans out to **one specialist per network** over gossipsub and synthesises their signed on-chain facts | one process per role |

Every message between nodes is **Ed25519-signed** by the sending agent's
identity, and every chain read is a **real JSON-RPC call** to a live endpoint.

> **Fully decentralised:** there is no shared chain and no broker. Each network
> is read by its own independent agent, and each reports its **genesis hash**
> (SVM) or **chain id** (EVM) — distinct per network, which proves the mesh is
> genuinely spanning different chains. Verified live across Solana testnet,
> Eclipse and Sonic (all SVM) plus Ethereum (2026-11-07):
>
> ```
> solana-testnet  slot 461623232  genesis 4uhcVJyU9pJkvQyS88uRDiswHXSCkY3zQawwpjk2NsNY
> eclipse         slot 200604771  genesis EAQLJCV2mh23BsK2P9oYpV5CHVLDNHTxYss3URrNmg3s
> sonic-testnet   slot 114036908  genesis BRSVhduW2sGkyhrxK6MYqyymirEggFv1hQh8jPqdYBLH
> ethereum        block 26362144  chain id 1
> ```

---

## How the agent talks to a chain

Chain access is packaged as `corrosive_agents` **skills** (JSON-in / JSON-out
tools). A skill is a thin wrapper over a JSON-RPC call, **bound to one endpoint**:

| Skill | Family | RPC method |
|-------|--------|-----------|
| `solana_get_slot` | SVM | `getSlot` |
| `solana_get_genesis_hash` | SVM | `getGenesisHash` |
| `solana_get_balance` | SVM | `getBalance` |
| `solana_request_airdrop` | SVM (devnet/testnet) | `requestAirdrop` |
| `evm_get_block_number` | EVM | `eth_blockNumber` |
| `evm_get_chain_id` | EVM | `eth_chainId` |
| `evm_get_balance` | EVM | `eth_getBalance` |

In **`solo`** mode these are handed to the model as tools and it calls them on
its own (`agent.chat_with_tools`). In the **mesh**, each specialist invokes its
own skills deterministically (`agent.skills().execute(...)`) and returns the
facts — so the on-chain data flows even with no LLM key.

---

## Architecture (mesh)

```
                 ┌───────────────────────────────────────────────┐
                 │                 libp2p mesh                    │
                 │   TCP · noise(XX) · yamux · gossipsub · mDNS    │
                 └───────────────────────────────────────────────┘

                                    signed tasks
   ┌────────┐ signed request ┌──────────────┐ ───────────────▶ ┌───────────────────┐
   │ client │ ─────────────▶ │ orchestrator │                  │ solana-testnet     │─▶ Solana testnet
   │        │                │  (LLM synth) │ ───────────────▶ │ eclipse   (SVM)    │─▶ Eclipse
   │        │ ◀───────────── │              │ ───────────────▶ │ sonic-testnet(SVM) │─▶ Sonic
   └────────┘ signed answer  └──────────────┘ ◀─────────────── │ ethereum  (EVM)    │─▶ Ethereum
                                    signed facts               └───────────────────┘
```

1. Each node generates an **Ed25519 agent identity** (`did:key`) and a libp2p
   node key; peers **pin** the `did:key` they learn at discovery.
2. The `client` discovers an `orchestrator` by its gossip announcement and sends
   a **signed** question.
3. The `orchestrator` fans the question out as **signed tasks** to *every*
   discovered specialist — one per network.
4. Each specialist reads its chain over JSON-RPC and gossips **signed facts**
   back (slot/block, genesis hash/chain id, balances). Every signature is
   verified.
5. The `orchestrator` synthesises one answer (NVIDIA Nemotron) attributing each
   figure to its network, and returns it **signed**; the client verifies it
   against the pinned key.

The specialist set is **dynamic** — start or stop a network's node and the mesh
adapts; nothing is hard-wired.

---

## Prerequisites

- **Rust** (stable; MSRV 1.88, matching the parent crate).
- A **free NVIDIA API key** from <https://build.nvidia.com> — required for the
  `solo` demo and the orchestrator's synthesis. *Optional* for the specialists
  (the mesh still produces on-chain facts without it). Put it in the
  repository-root `.env`:

  ```sh
  echo 'NVIDIA_API_KEY=nvapi-...' >> ../../.env
  ```

- No key is needed for the blockchains — the default endpoints are public.

Build once:

```sh
cargo build --release
```

---

## Quickstart — `solo` (one agent, tool-calling)

```sh
cargo run --release -- solo "What is the current Solana slot and the latest Ethereum block number?"
```

The agent calls the slot/block tools itself and answers from the results. Point
it at another network with `SOLANA_RPC_URL` / `EVM_RPC_URL`.

---

## Decentralised mesh — one terminal per network

Open terminals in this directory. **Start the orchestrator first** — it is the
bootstrap seed the others dial (works even where mDNS multicast is blocked).

```sh
# terminal 1 — coordinator + LLM synthesizer (bootstrap seed)
./run_role.sh orchestrator

# terminals 2..N — one specialist per Solana-like / EVM network
./run_role.sh solana-testnet
./run_role.sh eclipse            # another SVM chain
./run_role.sh sonic-testnet      # another SVM chain
./run_role.sh ethereum           # EVM (optional)

# last terminal — the user; discovers the orchestrator, asks, prints the answer
./run_role.sh client "Compare the slot and genesis hash of every network you can reach."
```

Each terminal logs `🔑` key creation, `🔒` encrypted channels, `🔎` discovery,
`🔏` signature verification, `⛓️` chain reads, and the `✅` verified final answer
in the client's pane.

### One command instead of many

```sh
./demo.sh
# or choose the networks:
CHAINS="solana-testnet eclipse" ./demo.sh "Which network has the higher slot?"
```

---

## Networks (presets)

| Preset | Family | Default endpoint |
|--------|--------|------------------|
| `solana-mainnet` | SVM | `https://api.mainnet-beta.solana.com` |
| `solana-testnet` | SVM | `https://api.testnet.solana.com` |
| `solana-devnet` | SVM | `https://api.devnet.solana.com` |
| `eclipse` | SVM | `https://mainnetbeta-rpc.eclipse.xyz` |
| `sonic-testnet` | SVM | `https://api.testnet.sonic.game` |
| `ethereum` | EVM | `https://ethereum-rpc.publicnode.com` |
| `base` | EVM | `https://base-rpc.publicnode.com` |
| `polygon` | EVM | `https://polygon-bor-rpc.publicnode.com` |

**Custom chain** — any SVM or EVM endpoint, including a local validator:

```sh
# a local solana-test-validator
ROLE=local-svm CHAIN_KIND=svm RPC_URL=http://127.0.0.1:8899 ./run_role.sh local-svm
# any other EVM
ROLE=arbitrum CHAIN_KIND=evm RPC_URL=https://arb1.arbitrum.io/rpc ./run_role.sh arbitrum
```

`RPC_URL` also overrides a preset's default endpoint (e.g. your own provider).

---

## Docker Compose

mDNS works across the Docker bridge, so the containers discover each other with
no seed list. The bundled compose runs Solana testnet + Eclipse + Sonic + Ethereum:

```sh
docker compose up --build
# watch the `sa-client` logs for the verified answer
```

---

## Environment variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `ROLE` | — | `solo` \| `orchestrator` \| `client` \| a network name |
| `RPC_URL` | preset | Override/set the specialist's endpoint |
| `CHAIN_KIND` | `svm` | `svm` \| `evm` (custom chains only) |
| `NVIDIA_API_KEY` | — | NVIDIA NIM key (solo demo + synthesis) |
| `NVIDIA_MODEL` | `nvidia/nemotron-3.5-bolt-e4b` | Chat model id |
| `SOLANA_RPC_URL` / `EVM_RPC_URL` | devnet / ethereum | `solo` demo endpoints |
| `CHAINS` | `solana-testnet eclipse sonic-testnet ethereum` | Networks `demo.sh` spawns |
| `QUESTION` | built-in | The client/solo question |
| `LISTEN` / `BOOTSTRAP` | mDNS | libp2p transport seed (set by the scripts) |
| `RUST_LOG` | `info,solana_agents=info` | Log verbosity |

See [`.env.example`](.env.example) for the full annotated list.

---

## Safety notes

- **Read-only by default.** The skills read chain state. The only state-changing
  operation is `solana_request_airdrop`, which works on **devnet/testnet only**
  and needs no private key — mainnet rejects it. No wallet keys are handled
  here, and nothing signs or sends a value transaction.
- **Message authenticity.** Requests, tasks, results and the final answer are
  all Ed25519-signed and verified against the `did:key` pinned at discovery.
- **Endpoints stay local.** Only a network's *name* and *kind* are gossiped, not
  its RPC URL — so an API-keyed provider URL never leaves your node.
- This use case has its **own workspace** and is **excluded** from the published
  `corrosive_agents` crate, so its libp2p/RPC dependency tree never ships to
  crates.io.
