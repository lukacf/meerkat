<!-- Release notes generated using configuration in .github/release.yml at v0.9.15 -->



**Full Changelog**: https://github.com/lukacf/meerkat/compare/v0.9.14...v0.9.15

<!-- Release notes generated using configuration in .github/release.yml at v0.9.15 -->



**Full Changelog**: https://github.com/lukacf/meerkat/compare/v0.9.14...v0.9.15

## Known limitations

The known limitations of Meerkat 0.9.15 are in the release's own notes: section 0.9.15, Known limitations, of CHANGELOG.md at this tag (https://github.com/lukacf/meerkat/blob/v0.9.15/CHANGELOG.md). The two entries below were added after the release; they are in CHANGELOG.md on main, not at the tag.

- Added 2026-11-16, after the release. External events admitted without a
  wake (REST `POST /sessions/{id}/external-events`, JSON-RPC
  `session/external_event`): such an event waits queued for the session's
  next turn only while the session is idle. Admitted over REST while a turn
  is running, it is queued behind that turn, and the runtime takes it at
  once when the turn ends, as a new turn: at least one model call, in a run
  that the caller did not start. The same happens in the instant after a
  turn's call has answered: after a run ends the runtime looks once more for
  queued input before it comes to rest, and the caller's answer is released
  before that look. The admission answers that the event is queued in every
  case (REST `202`). Nothing is lost: the event is taken once and reaches
  one terminal state; a run that fails before completing retries it, as for
  any queued input. To keep an event for the next turn, post it while the
  session is idle; how long after a turn's answer a session counts as idle
  is not established. The running turn and the idle session are by a run on
  the released 0.9.15 `rkat-rest`; the instant after a turn's answer is read
  from the source and seen as one `meerkat-rest` test that fails in some
  runs on a machine with one to four CPUs. On JSON-RPC the event is admitted
  only after the running turn has ended; whether it is then taken at once
  was not run. The same lines stand at every tag from 0.9.11 to 0.9.15;
  earlier releases were not read.

- Added 2026-11-16, after the release. `rkat-rest`: where the server's
  default context directory does not exist yet (on Linux by default
  `~/.local/share/meerkat/rest`, or `$XDG_DATA_HOME/meerkat/rest` when that
  is set; a new machine or a new account), a `POST /sessions` that carries
  no `system_prompt` is answered HTTP 500, every time: `agent error: Build
  error: Config error: project AGENTS.md '<path>' is not readable: No such
  file or directory (os error 2)`, where `<path>` is that directory. The
  server does not create it; the directory the server is started from does
  not matter, and setting XDG_DATA_HOME only moves the directory to
  `$XDG_DATA_HOME/meerkat/rest`. Create the directory the message names (on
  Linux by default `mkdir -p ~/.local/share/meerkat/rest`; a running server
  need not be restarted), or start the server with
  `--context-root <an existing directory>`. A request that carries a
  `system_prompt` is not affected. Affects 0.7.9 through 0.9.15: the lines
  that cause it came with 0.7.9 and stand at every tag from 0.8.0 (read from
  the source); run on the released Linux x86_64 `rkat-rest` with an empty
  home directory for 0.8.28, 0.8.52, 0.9.0, 0.9.13, 0.9.14 and 0.9.15.
  `rkat-rpc`, the server the Python and TypeScript SDKs start, is not
  affected (run on 0.9.14 and 0.9.15); `rkat-mcp`, the `rkat` command and
  the Meta gateway take their context directory from elsewhere and are not
  affected, by a reading of the source. macOS and Windows were not run. A
  fix is planned for the next release.

- Added 2026-11-17, after the release. A `[confinement]` table in a project's `.rkat/config.toml` is not read, and `rkat` does not warn about it: an agent's shell is not confined by it. State confinement in the user document, `~/.rkat/config.toml`, as `[confinement.all]` with `mode = "workspace"`, so that it covers every launch kind; do not rely on a realm document as its only statement. The `--confinement` flags are for the one command that carries them. Whichever you use, check that command's stderr for the confinement lines: three, one for each launch kind, when a requirement applies, and none when none does; where none is printed, the command is not confined. On Linux a start that confines every launch kind while the realm state lies inside the directory the command is started in, which is the default with `rkat`, is refused and asks for `--state-root <dir>` outside the project. The section "Confinement for one project" of the CLI configuration page says what else changes under confinement. (Reported by a user on this release on macOS; shown by the maintainers' runs on 0.9.16 on Linux x86_64; not run on 0.9.15.) A fix is planned.

- Added 2026-11-17, after the release. Peer requests: on a host whose realm is on disk, a session that has sent a peer request stops running when a response to that request arrives without a result; after the next restart of the host the session can be unable to run for good. This release has it too (shown on a build of the 0.9.15 tag: stopped in 5 of 5 with `accepted` without a result and in 5 of 5 with `failed` without a result, not stopped in 5 of 5 with a result; and by runs of the published Meta `rpc_gateway` 0.9.15). The entry of the same date on the 0.9.16 release page (https://github.com/lukacf/meerkat/releases/tag/v0.9.16) says which hosts are affected and what is seen; it applies to this release as written.
