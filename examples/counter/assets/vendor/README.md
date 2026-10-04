# Vendored Phoenix client

Third-party code, committed unmodified (the unmodified client is what Griffin conforms to). Never edit these files.

| File | Package | Path in the package | SHA-256 |
|---|---|---|---|
| `phoenix.mjs` | npm `phoenix` 1.8.15 | `priv/static/phoenix.mjs` | `a97d00dd7e968f40b3bcc060490f43360f725794e6d79029f413da3ac4c8e66c` |
| `phoenix_live_view.esm.js` | npm `phoenix_live_view` 1.2.12 | `priv/static/phoenix_live_view.esm.js` | `4d844e766c1cf39656835762bacbe45c9c56596886b6af512f8e0c823bf65527` |

They are the prebuilt ES module bundles those releases ship, the same bytes as at the git tags of `phoenixframework/phoenix` (`v1.8.15`) and `phoenixframework/phoenix_live_view` (`v1.2.12`). Both are MIT licensed; `LICENSE-phoenix.md` and `LICENSE-phoenix_live_view.md` are each package's `LICENSE.md`. The source maps the bundles name in their last line are not vendored.

## Upgrading

Do this as part of upgrading the pinned version (`crates/griffin-web/tests/conformance/README.md`), never by itself: the server answers a join with the version in `CLIENT_VERSION` (`crates/griffin-web/src/live.rs`), and the client warns when its own differs.

```bash
npm pack phoenix@<version> phoenix_live_view@<version>
tar xzf phoenix-<version>.tgz                # package/priv/static/phoenix.mjs, package/LICENSE.md
tar xzf phoenix_live_view-<version>.tgz      # package/priv/static/phoenix_live_view.esm.js, package/LICENSE.md
```

Copy the two bundles and the two licenses over the files here and over the same four files in `crates/cargo-griffin/vendor/` (the copy `cargo griffin new` writes into projects; `the_client_is_vendored_byte_for_byte_from_the_one_copy_in_the_repository` fails if they differ), update the table (`shasum -a 256`), then run the browser tests (`browser-tests/README.md`).
