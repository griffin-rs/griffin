# Regenerates fixtures/<case>.json from cases/<case>.json by running each case
# through real Phoenix LiveView. See README.md in this directory.
#
#     elixir crates/griffin-web/tests/conformance/generate.exs

# The pinned client release. Exact versions, never ranges.
pins = [phoenix_live_view: "1.2.12", phoenix: "1.8.15"]

Mix.install(pins ++ [{:jason, "~> 1.4"}])

for {app, version} <- pins do
  loaded = to_string(Application.spec(app, :vsn))

  if loaded != version do
    raise "conformance fixtures must come from #{app} #{version}, but #{loaded} is loaded"
  end
end

defmodule Conformance do
  alias Phoenix.LiveView.{Diff, Socket, Utils}

  # A case of client commands: each is Elixir source that builds a `JS`. Returns, for
  # each, the operation list as the client reads it from the attribute.
  def run(_name, %{"commands" => commands}) do
    Map.new(commands, fn {name, source} ->
      {js, _binding} = Code.eval_string("alias Phoenix.LiveView.JS\n" <> source)
      {name, wire(js)}
    end)
  end

  # Returns the diff Phoenix emits after each step, as the wire JSON decoded back to maps.
  def run(name, %{"template" => template, "steps" => steps} = test_case) do
    view = compile(name, template, Map.get(test_case, "components", ""))
    state = {%Socket{}, Diff.new_fingerprints(), Diff.new_components()}

    {diffs, _state} =
      Enum.map_reduce(steps, state, fn step, {socket, fingerprints, components} ->
        # What a callback did besides assigning: `push_event/3`, and `{:reply, map, socket}`.
        {effects, step} = Map.split(step, ["push_events", "reply"])
        # Existing atoms only: a step key the compiled template never mentions is a typo.
        assigns = Map.new(step, fn {k, v} -> {String.to_existing_atom(k), v} end)
        socket = Phoenix.Component.assign(socket, assigns)

        socket =
          Enum.reduce(effects["push_events"] || [], socket, fn [event, payload], socket ->
            Phoenix.LiveView.push_event(socket, event, payload)
          end)

        socket = if reply = effects["reply"], do: Utils.put_reply(socket, reply), else: socket
        rendered = view.render(socket.assigns)
        {diff, fingerprints, components} = Diff.render(socket, rendered, fingerprints, components)
        # As the channel does after every render (`render_diff` in channel.ex).
        diff = Diff.render_private(socket, diff)
        socket = socket |> Utils.clear_changed() |> Utils.clear_temp()
        {wire(diff), {socket, fingerprints, components}}
      end)

    diffs
  end

  # Same as writing the template in a ~H sigil inside a function component.
  # `components` is Elixir source defining the function components the template calls.
  defp compile(name, template, components) do
    body = {:sigil_H, [], [{:<<>>, [], [template]}, []]}

    contents =
      quote do
        use Phoenix.Component
        unquote(Code.string_to_quoted!(components))
        def render(var!(assigns)), do: unquote(body)
      end

    {:module, view, _, _} =
      Module.create(Module.concat(__MODULE__, Macro.camelize(name)), contents, Macro.Env.location(__ENV__))

    view
  end

  # Encode exactly as the Phoenix socket serializer does, then decode.
  defp wire(diff), do: diff |> Phoenix.json_library().encode!() |> Jason.decode!()

  # Sorted keys, so regenerating never reorders a fixture.
  def sorted(%{} = map),
    do: map |> Enum.sort() |> Enum.map(fn {k, v} -> {k, sorted(v)} end) |> Jason.OrderedObject.new()

  def sorted(list) when is_list(list), do: Enum.map(list, &sorted/1)
  def sorted(other), do: other
end

for path <- Path.wildcard(Path.join(__DIR__, "cases/*.json")) do
  name = Path.basename(path, ".json")
  diffs = Conformance.run(name, path |> File.read!() |> Jason.decode!())
  fixture = Path.join([__DIR__, "fixtures", name <> ".json"])
  File.write!(fixture, [Jason.encode!(Conformance.sorted(diffs), pretty: true), "\n"])
  IO.puts("wrote #{Path.relative_to_cwd(fixture)}")
end
