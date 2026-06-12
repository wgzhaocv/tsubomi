# tsubomi 蕾

A Cargo workspace + React frontend.

```
tsubomi/
├── Cargo.toml              # workspace (resolver 3, release profile)
├── crates/
│   ├── shared/             # tsubomi-shared — serde types shared by server + cli
│   ├── server/             # tsubomi-server — axum HTTP API (bin)
│   └── cli/                # tsubomi-cli — clap client (bin name: `tsubomi`)
├── web/                    # Vite (vite-plus / `vp`) + React + TS + Tailwind v4 + shadcn
└── justfile
```

## Prerequisites

- Rust (pinned to 1.95 via `rust-toolchain.toml`)
- [bun](https://bun.sh) for the frontend
- [just](https://github.com/casey/just) (optional, for the recipes below)

## Develop

```bash
just web-install         # first time only — install web deps
just dev                 # backend (:8080) + frontend (:5173) together; Ctrl-C stops both
```

Then, in another terminal, drive the CLI:

```bash
just cli hello           # or: cargo run -p tsubomi-cli -- hello
just cli health
```

Need the two halves separately? `just dev-server` and `just dev-web`.

The CLI's server URL defaults to `http://localhost:8080`; override with
`--server <url>` or the `TSUBOMI_SERVER` env var.

## API

| Method | Path          | Response                       |
| ------ | ------------- | ------------------------------ |
| GET    | `/api/health` | `{ status, version }`          |
| GET    | `/api/hello`  | `{ message }`                  |

Both shapes live in `crates/shared` so the server and CLI share one contract.

## Build

```bash
just build               # release binaries + production web bundle (web/dist)
```

## Add dependencies

Rust deps go in via `cargo add` (never hand-edit `[dependencies]`):

```bash
cargo add -p tsubomi-server <crate>
cargo add -p tsubomi-cli <crate>
```

shadcn components:

```bash
cd web && bunx shadcn@latest add button
```
