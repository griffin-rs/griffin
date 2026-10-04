# __App__

A Griffin application: a Rust web app on axum that speaks the Phoenix LiveView protocol.

```bash
cargo griffin dev             # http://127.0.0.1:4000, rebuilds and reloads on save
cargo griffin routes          # the route table
cargo run -p __app___web      # the app alone, on the port in config.toml
cargo test --workspace
```

`config.toml` was created with a secret key and is not committed. Set the same
settings in production with environment variables (see `config.example.toml`).

## Layout

| Where | What | Phoenix |
|---|---|---|
| `crates/__app__/` | the domain: Contexts, Changesets, Capability traits. Depends on `griffin-domain` only, so it cannot import web code | contexts |
| `crates/__app___web/src/routes.rs` | the one route table | router |
| `crates/__app___web/src/pages.rs` | controllers | controllers |
| `crates/__app___web/src/live.rs` | LiveViews | live views |
| `crates/__app___web/src/components.rs` | input and label Components | core components |
| `crates/__app___web/src/layouts.rs` | the root layout and error pages | layouts |
| `crates/__app___web/src/state.rs` | the application state: implements the Capabilities | endpoint / application |
| `crates/__app___web/templates/` | `.html.griffin` template files | templates |
| `crates/__app___web/assets/` | JavaScript, CSS and the vendored Phoenix client: the input of esbuild and Tailwind | assets |
| `crates/__app___web/static/` | what the server serves as files; `static/assets/` is built from `assets/` and not committed | priv/static |
| `xtask/` | `cargo griffin <command>` | mix tasks |

The web crate names Griffin `griffin` in its `Cargo.toml` (`package = "griffin-web"`).

## Release builds

`cargo build --release`, and nothing else: never `--all-features` and never `--features
dev`. The `dev` feature adds the browser reload and the handed-over socket, and only
`cargo griffin dev` should turn it on. Build `static/assets/` first (the app refuses to
start without `app.js` and `app.css`).

## Security defaults

Every browser route runs the browser Pipeline: secure headers, signed and encrypted
session, CSRF check. The socket needs the page's CSRF token and a same-origin request.
Nothing is turned off here; to change a default, do it explicitly in `src/lib.rs`.

## The vendored client

`assets/vendor/` holds the Phoenix JavaScript client at the versions this Griffin
release speaks (`phoenix` 1.8.15, `phoenix_live_view` 1.2.12), unmodified. Do not edit
them or mix versions.
