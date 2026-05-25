# caveman-dashboard

Local savings tracker for the [caveman](https://github.com/JuliusBrussee/caveman)
Claude Code skill. Reads your `~/.claude/projects/**/*.jsonl` transcripts,
applies the published compression factor, and shows per-day + cumulative
USD savings. Optional read-only integration with SovaCount for combined totals.

## Run

    cargo run --release
    open http://127.0.0.1:8991

## What it does

- Parses every assistant-message in `~/.claude/projects/`
- Multiplies output-tokens by Anthropic's public per-million price
- Applies the 65% compression mean from caveman's benchmark
- Sums per-day and cumulative
- Optional `+ show sovacount` button: if `http://127.0.0.1:8989/cost` is reachable,
  adds those savings into a combined figure (read-only — never writes to sovacount)

## What it doesn't do

- No write-back to caveman, sovacount, or transcripts
- No telemetry, no remote calls, no analytics
- No mutations: pure read-only consumer
- No model invocations: 100% offline transcript-parsing

## License

MIT. Built as a companion tool to caveman by @JuliusBrussee — not affiliated.
