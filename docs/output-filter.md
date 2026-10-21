# Global accessibility text filter

The `output_filter` setting enables deterministic regex filtering in this fork.
It is off when the base user configuration has no rules. Add rules to
`~/.codex/config.toml`, outside any profile, and restart the Codex process.
Projects, profiles, and session flags cannot replace or disable this policy.
This feature is part of the runtime and does not depend on hook trust.

```toml
[[output_filter.rules]]
pattern = '(?i)\bsecret\w*\b'
exempt_pattern = '(?i)secretar(?:y|ies)'
replacement = '[filtered]'
```

This example filters a sample word family while allowing two unrelated words.
Substitute your own patterns locally. An exemption must match the entire regex
candidate; an exempt word elsewhere in a message does not exempt other text.
Matching uses `regex-lite` syntax. Lookaround and backreferences are unsupported.
Use TOML literal strings for regexes. Replacements are literal strings, including
any dollar signs. Invalid patterns, patterns matching empty input, and unsafe
replacement strings produce a generic configuration error without quoting them.
Replacement results are rechecked against every rule. A result that cannot be
made safe after eight passes is omitted.

## Covered text paths

- Session messages, commentary, visible reasoning summaries and raw reasoning
  events, tool progress, errors, warnings, and completed items are filtered before
  delivery. Internal model reasoning that is never exposed is outside this layer.
- Streaming deltas are withheld. Complete item events carry filtered text, so a
  word split over multiple network chunks cannot bypass the check.
- App-server transport serialization filters notifications and RPC responses,
  including historical content loaded through RPC. Protocol identifiers and keys
  retain their original values; an envelope is suppressed if those values match.
- Session rollout appends filter their serialized text. Existing files are not
  rewritten. A typed record that cannot retain a valid schema is not appended.
- Terminal frames are checked across cell and style boundaries. Matching cells
  are blanked while preserving layout. Scrollback is filtered before wrapping;
  unfiltered reflow sources and terminal hyperlink metadata are removed.
- Native patches validate the complete resulting file and destination paths
  before applying any change. Their filesystem adapter also validates the actual
  final write buffer after approval, in case a file changes during the wait.
  App-server file writes validate decoded UTF-8
  content. App-server text reads are filtered before base64 encoding. Exec JSON
  output, last-message files, and plain-text and HTML clipboard copies use the
  same policy.

## Restricted operations

Filtering executable text could change the behavior of code. Tool arguments and
file writes are rejected on a match so the model can rephrase them, rather than
silently modifying a program. Checks run again after hooks replace tool arguments.

While enabled, model-dispatched tools are restricted to native patches, native
text reads, directory listing, text search, planning, and user questions. Shells,
code execution, external tools, and media tools are blocked because they can
generate file content indirectly. App-server binary file reads, binary file
writes, and unvalidated copies are blocked. Inline transcript images are hidden
because a text regex cannot validate pixels.

## Boundaries

This is a text accessibility feature, not an operating-system write monitor.
Independent programs, pre-existing files, external clients' own UI, rasterized
text, audio, direct diagnostics outside the covered writers, and independently
running hook or plugin processes are not covered. Separately configured processes
can still write files. Do not treat this implementation as an absolute guarantee
that every byte on the computer is filtered. It does not alter the currently
running installed binary; build and launch this fork to use it.

## Validation

Run from `codex-rs`:

```sh
cargo test -p codex-config output_filter
cargo test -p codex-core output_filter
cargo test -p codex-tui output_filter
cargo check -p codex-app-server -p codex-exec
cargo run -p codex-config-schema --bin codex-write-config-schema
cargo fmt --check
```

Regression cases cover inflections, case, exact exemptions, nested content,
protocol identity, safe error text, literal replacements, replacement-created
matches, chunk boundaries, global configuration precedence, and terminal cells.
