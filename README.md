# Griffin

> A batteries-included web framework for Rust, inspired by Elixir's Phoenix.

Griffin aims to give Rust developers what Phoenix gives Elixir developers: one coherent framework that ships with everything a real web application needs, so you spend your time building the product rather than gluing libraries together.

---

## Why Griffin?

The Rust web ecosystem has excellent building blocks: fast HTTP servers, async runtimes, database drivers, template engines. What it lacks is a single, opinionated home for them. Starting a new project means picking a dozen crates, wiring them together, and making the same decisions every team has made before.

Phoenix showed that a framework can be productive and fast at the same time. It gives you real-time features by default, server-rendered interactive UIs without a JavaScript framework, and a clear way to organise an application. Griffin brings that philosophy to Rust and adds what Rust does well: safety, performance and compile-time guarantees.

## Vision

- **One framework, one way.** Sensible defaults and conventions, with escape hatches when you need them.
- **Real-time by default.** Live, interactive pages should be the normal case, not a special project.
- **Everything included.** Mail, background jobs, scheduling, state machines, auth, and more are part of the framework, not left to third parties.
- **Compile-time confidence.** Templates, routes and data shapes are checked before your app runs.
- **Fast feedback.** Generators, live reload and clear error pages make development enjoyable.
- **Production ready.** Observability, configuration and deployment are first-class concerns.

## Guiding Principles (borrowed from Phoenix)

| Phoenix idea                      | What it means for Griffin                                                                                                                      |
|-----------------------------------|------------------------------------------------------------------------------------------------------------------------------------------------|
| **LiveView**                      | Rich, interactive UIs rendered on the server and kept in sync with the browser over a persistent connection. Most of your logic stays in Rust. |
| **HEEx templates**                | HTML-aware templates that are validated at compile time, with reusable function components and slots.                                          |
| **Contexts**                      | Business logic grouped into clear domain boundaries, kept apart from the web layer.                                                            |
| **Plugs / pipelines**             | Requests flow through small, composable steps, such as auth, sessions or rate limiting, arranged into named pipelines.                         |
| **Channels & PubSub**             | Built-in real-time messaging between server and clients, and between parts of the app.                                                         |
| **Presence**                      | Track who is online and what they are doing, across a cluster.                                                                                 |
| **Supervision & fault tolerance** | Long-running work is isolated and supervised, so one failure doesn't take the app down.                                                        |
| **Generators**                    | Scaffold resources, auth, live pages and more from the command line.                                                                           |

## What's in the Box

### Web Layer
- **Router**: expressive routes, scopes, pipelines and path helpers that the compiler checks.
- **Controllers**: classic request/response handlers for pages and APIs.
- **LiveView**: stateful, server-driven interactive pages with events, forms, uploads, navigation and streams.
- **Components**: reusable UI building blocks with typed attributes and slots.
- **Templates**: a HEEx-style template language for HTML, checked at compile time.
- **Forms & Validation**: form helpers tied to data validation, with friendly error messages.
- **Static Assets**: an asset pipeline with live reload during development.
- **JSON APIs**: first-class support for building APIs alongside HTML pages.

### Real-time
- **Channels**: two-way messaging over WebSockets with topics and authorization.
- **PubSub**: publish and subscribe within a node and across a cluster.
- **Presence**: distributed tracking of connected users.

### Data
- **Database layer**: schemas, queries, migrations and transactions.
- **Changesets**: a structured way to filter, cast and validate data before it is persisted.
- **Seeds & Fixtures**: easy setup of development and test data.

### Background Work
- **Job Queue**: durable background jobs with retries, priorities, uniqueness and backoff.
- **Scheduler**: recurring jobs on a cron-like schedule.
- **Workflows / State Machines**: model processes such as orders, approvals or onboarding as explicit states and transitions, with history and guards.

### Communication
- **Mailer**: compose emails with templates, preview them in development, and deliver them through pluggable providers.
- **Notifications**: one way to reach users by email, in-app, push or webhook.

### Security & Identity
- **Authentication**: a generator for a complete auth system covering registration, login, sessions, password reset and email confirmation.
- **Authorization**: policies for deciding who may do what.
- **Built-in protections**: CSRF, secure headers, safe-by-default HTML escaping and rate limiting.

### Platform
- **Configuration**: environment-aware config with runtime secrets.
- **Caching**: a simple, unified caching interface.
- **File Storage & Uploads**: local and cloud storage behind one interface.
- **Internationalization**: translations and locale handling.
- **Telemetry & Observability**: structured logging, metrics and tracing hooks throughout the framework.
- **Live Dashboard**: a built-in admin view of requests, jobs, queues, processes and system health.
- **Testing Tools**: helpers for testing controllers, LiveViews, channels, jobs and mail.
- **CLI & Generators**: create projects, scaffold features, run migrations and manage the app.
- **Deployment**: build a single, self-contained release that is easy to ship.

## Who Is It For?

- Rust developers who want to build full web products, not just APIs.
- Phoenix and Rails developers who want Rust's performance and safety without giving up productivity.
- Teams who would rather follow strong conventions than reinvent their stack on every project.

## Planned phases (high level)

1. **Foundation**: HTTP core, router, pipelines, controllers, templates.
2. **Interactivity**: LiveView, components, channels, PubSub.
3. **Data**: database layer, changesets, migrations, generators.
4. **Background & Communication**: job queue, scheduler, mailer, state machines.
5. **Identity**: auth generator, authorization, security defaults.
6. **Operations**: telemetry, live dashboard, releases and deployment.
7. **Ecosystem**: documentation, guides, example apps and community plugins.

## Status

Phase 1 (web layer and LiveView, no database) is built; everything else in "What's in the Box" is still a plan, tracked per phase in the [milestones](https://github.com/griffin-rs/griffin/milestones). Griffin is not released: nothing is on crates.io, the API will change, and it has had no production use.

What works today:

- **`cargo griffin new`, `dev` and `routes`.** `new` checks your Rust toolchain, downloads standalone esbuild and Tailwind (checked against pinned SHA-256 sums; `--no-tailwind` skips Tailwind), and generates a workspace of a domain crate, a web crate and an `xtask` crate, with the Phoenix JavaScript client vendored at the version Griffin speaks. `dev` rebuilds and restarts on save while holding the socket open, reloads the browser, and keeps serving the last good build when a compile fails.
- **Router.** One `routes!` table: scopes, pipelines, layouts, verb routes, LiveView routes, live sessions, and Path helpers the compiler checks. A plain builder API does the same without the macro.
- **Controllers.** Plain axum handlers; a template, an outcome enum or JSON as the return value; flash; one place that maps your error type to a response.
- **Templates and Components.** `html!` and `.html.griffin` files: Rust expressions checked by rustc, `@assign`, `:if`, `:for`, escaping by default, Components with typed attributes and slots, errors reported at the template's line and column.
- **LiveView.** Dead and Connected render, Events and Messages handled one at a time, diffs sent for only what changed in the Phoenix wire format (checked against fixtures captured from Phoenix LiveView 1.2.12; the client is phoenix 1.8.15), client commands, hooks, patch and navigate, live sessions, and failure isolation: one LiveView that fails ends alone and is remounted clean.
- **Forms.** Changesets that parse input into domain types, shared by live forms and controller forms.
- **Security defaults.** Signed and encrypted cookie session, CSRF token on forms and on the socket, origin check, signed expiring render tokens, secure headers, typed configuration that refuses to boot when a setting is missing.

What does not exist yet: a database, authentication, PubSub, Channels, Presence, streams, uploads, stateful LiveComponents, background jobs, mail, other generators than `new`, release tooling, and a crates.io release. Compile-time change tracking and static hot reload are later work: a LiveView compares every slot after each Event today. The tool downloads are pinned for macOS and Linux (x86_64 and arm64); there are no Windows pins yet, so `new` stops there.

### Try it

You need a stable Rust toolchain at or above the `rust-version` in [Cargo.toml](Cargo.toml), and network access once, for the tool downloads. Griffin is unreleased, so install the command from a checkout and point new projects at it:

```bash
git clone https://github.com/griffin-rs/griffin
cargo install --path griffin/cargo-griffin
cargo griffin new my_app --griffin-path "$PWD/griffin"
cd my_app
cargo griffin dev            # http://127.0.0.1:4000
```

Open <http://127.0.0.1:4000>. Edit `crates/my_app_web/templates/home.html.griffin` and save: the app rebuilds and the open page reloads. `/signup` is a live form validated as you type, and `/signup/classic` is the same Changeset behind a plain form post. `cargo griffin routes` prints the route table, and the generated `README.md` explains the layout. Inside a project `cargo griffin` runs through a cargo alias, so the project always uses its own Griffin version. Cargo prints a warning that the user-defined alias `griffin` is shadowing an external subcommand on every such call, because the installed `cargo-griffin` has the same name; it is harmless and tracked as #66 (cargo issue 10049).

Working on Griffin itself: see [CONTRIBUTING.md](CONTRIBUTING.md).

## Inspiration

- [Phoenix Framework](https://www.phoenixframework.org/)
- [Phoenix LiveView](https://github.com/phoenixframework/phoenix_live_view)
- [Ruby on Rails](https://rubyonrails.org/), [Laravel](https://laravel.com/) and [Django](https://www.djangoproject.com/), for the batteries-included approach

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
