# Conformance with Phoenix LiveView

Griffin must emit the same diff JSON as the pinned Phoenix LiveView release (the wire protocol is Phoenix's, unchanged). Each case is run through real Phoenix once, the result is committed, and the Rust tests compare Griffin's output with it.

- `cases/<name>.json`: the case, written by hand. `template` is HEEx source; `steps` is a list of assigns to set, one render per step. An optional `components` is Elixir source defining the function components the template calls as `<.name />`.
  A step may also say what a callback did besides assigning: `push_events`, a list of `[name, payload]` given to `push_event/3`, and `reply`, the map of a `{:reply, map, socket}`. They are not assigns. The fixture then has them as the channel adds them to the diff (`"e"`, `"r"`).
  A case with `commands` instead of a template is a map from a name to Elixir source that builds a `Phoenix.LiveView.JS` (aliased as `JS`). Its fixture is a map from each name to the operation list the client reads from the attribute.
- `fixtures/<name>.json`: the diff Phoenix emitted after each step. Generated; never edit by hand.
- `generate.exs`: regenerates every fixture from its case.
- `main.rs`: the Rust tests and the comparison helper.

Running the tests needs only Rust:

```bash
cargo test -p griffin-web --test conformance
```

Regenerating fixtures needs Elixir and network access to Hex:

```bash
elixir griffin-web/tests/conformance/generate.exs
```

## Adding a case

1. Write `cases/<name>.json`. Take the case from Phoenix's own `diff_test.exs` where possible. A Rust `if` or `match` with an `html!` in each branch corresponds to an Elixir expression with a `~H` in each branch; make the outer expression read the assigns the inner templates use (`{@name && if @show, do: ~H|...|}`), because Phoenix only re-evaluates an expression when an assign it reads itself has changed.
   Griffin's `:for={item in items}` is HEEx's `:for={item <- @items}`. Give each step the whole list: Phoenix compares a list assign by value, so a step that sets an equal list re-renders nothing.
   A Griffin Component is a Phoenix function component: put its `attr` declarations and `def` in the case's `components`. Phoenix writes the extra attributes of `attr :rest, :global` in the order of its map, not the order given, so give them to the Griffin tag in the order the fixture shows.
   A slot of a Griffin Component is a `slot` of the function component, `{slot}` and `{slot.render(argument)}` are `render_slot`, and `:let` is `:let`. Write the tag on one line, because the two trim whitespace around slot entries differently. Phoenix renders the content of a slot with `:let` again whenever anything it reads has changed, and then resends every expression in it that reads the argument, so pick steps that change all of them or none. Give the content of two slots different statics: Griffin names a template by its statics alone, and would share one position in `"p"` where Phoenix has two.
2. Run `generate.exs` and commit the new fixture with the case.
   A step with `push_events` or `reply` is what a LiveView's socket does, so its case is run at the socket seam: `pushed_events_and_reply` is compared in `tests/live_socket.rs`, frame by frame.
3. Add a test named `<name>` to `main.rs`: `diffs(name, |assigns| ...)` runs the case's steps through a diff state, with the closure building the case's template, with `html!` or by hand from Griffin's public template API, and `assert_matches_phoenix(name, &diffs)` compares the result with the fixture. A mismatch lists every differing key path.

## Upgrading the pinned version

1. Change the versions in `pins` at the top of `generate.exs`. They are exact; the script refuses to write fixtures if any other version is loaded.
2. Run `generate.exs`. The fixture changes are the protocol changes: review them in the diff.
3. Make the Rust tests pass again. The pin is also recorded in the docs; update them in the same change.
4. Replace the vendored client bundles and run the browser tests: see `examples/counter/assets/vendor/README.md`.
