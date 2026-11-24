# Findings fixtures

Each file is the `findings` input of one `findings.import` call: a JSON array of findings, as
`quadratique findings import --tool linter --findings "$(cat findings-1.json)"` reads it.

- `findings-1.json` is a complete run of a linter with two findings: `L204 f:49f3eaf2628a1f35`
  and `L420 f:a420556f407006e2`.
- `findings-2.json` is the same run without `L420 f:a420556f407006e2`: the second run, in which
  one finding is gone.

The trackers with no import of findings read the same files through a script
(`qeval/adapters/common.py`), which names each issue after the finding's key.
