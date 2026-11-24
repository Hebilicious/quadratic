from __future__ import annotations

import argparse
import json
import shutil
import time
from datetime import datetime
from pathlib import Path

from qeval import report, sandbox, scripted
from qeval.adapters import beads, lific, quadratic
from qeval.adapters.contract import Adapter

ADAPTERS: dict[str, Adapter] = {"quadratic": quadratic, "beads": beads, "lific": lific}
NO_RUN = ("qeval report needs a run to read: pass --run <run folder>, or --from agents=|scripted=|all=<run folder>[:tool,...] "
          "(repeat it) with --out <folder>")


def _save(run: Path, key: str, tool: str, value) -> None:
    path = run / "results.json"
    data = json.loads(path.read_text()) if path.exists() else {}
    data.setdefault(key, {})[tool] = value
    path.write_text(json.dumps(data, indent=2, default=str))


def run_scripted(run: Path, tools: list[str], rounds: int, repeats: int, quadratic_bin: Path) -> None:
    for name in tools:
        adapter = ADAPTERS[name]
        started = time.time()
        with scripted.Prepared(adapter, run / name / "scripted", quadratic_bin, worktrees=3) as p:
            _save(run, "setup", name, scripted.measure_setup(p))
            first = scripted.first_tokens(p)
            texts = first.pop("texts")
            for label, text in texts.items():
                (run / name / f"instructions-{label.replace(' ', '_').replace('/', '_')}.txt").write_text(text)
            _save(run, "tokens", name, first)
            _save(run, "human", name, {"default": scripted.human_question(p, distinct=False)})
        with scripted.Prepared(adapter, run / name / "merge", quadratic_bin) as p:
            _save(run, "merge", name, scripted.merge_and_deploy(p))
        with scripted.Prepared(adapter, run / name / "findings", quadratic_bin) as p:
            _save(run, "findings", name, scripted.findings(p))
        for distinct in (False, True):
            with scripted.Prepared(adapter, run / name / f"history-{'distinct' if distinct else 'default'}", quadratic_bin) as p:
                _save(run, f"history_{'distinct' if distinct else 'default'}", name, scripted.history(p, distinct))
        races = {}
        with scripted.Prepared(adapter, run / name / "races", quadratic_bin) as p:
            modes = [("default", False, False), ("distinct", True, False)]
            if name == "lific":
                modes.append(("expected_seq", False, True))
            for label, distinct, cas in modes:
                races[f"claims_{label}"] = [scripted.double_claims(p, rounds, distinct, cas) for _ in range(repeats)]
            races["append"] = [scripted.lost_updates(p, max(rounds // 2, 1), "append") for _ in range(repeats)]
            races["rewrite"] = [scripted.lost_updates(p, max(rounds // 2, 1), "rewrite") for _ in range(repeats)]
            if name == "lific":
                races["rewrite_expected_seq"] = [scripted.lost_updates(p, max(rounds // 2, 1), "rewrite", cas=True)
                                                 for _ in range(repeats)]
        _save(run, "races", name, races)
        print(f"{name}: scripted measures done in {time.time() - started:.0f} s", flush=True)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="qeval")
    sub = parser.add_subparsers(dest="command", required=True)
    s = sub.add_parser("scripted")
    s.add_argument("--rounds", type=int, default=50)
    s.add_argument("--repeats", type=int, default=3)
    s.add_argument("--tools", default="quadratic,beads,lific")
    s.add_argument("--run", type=Path)
    g = sub.add_parser("agents")
    g.add_argument("--run", type=Path, required=True)
    g.add_argument("--tools", default="quadratic,beads,lific")
    a = sub.add_parser("reanalyze")
    a.add_argument("--run", type=Path, required=True)
    a.add_argument("--tools", default="quadratic,beads,lific")
    r = sub.add_parser("report", help="one run folder (--run), or several (--from ... --out)")
    r.add_argument("--run", type=Path)
    r.add_argument("--from", dest="sources", action="append", default=[],
                   help="agents=, scripted= or all=<run folder>[:tool,...]; repeat it")
    r.add_argument("--out", type=Path)
    sub.add_parser("check-home")
    args = parser.parse_args(argv)

    if args.command == "check-home":
        found = sandbox.real_home_check()
        print(json.dumps(found, indent=2))
        return 1 if any(found.values()) else 0
    if args.command == "report":
        if args.run and (args.sources or args.out):
            raise SystemExit("qeval report: give either --run, or --from with --out, not both")
        if args.run:
            print(report.write(args.run))
        elif args.sources and args.out:
            specs = [report.parse_from(spec.replace("<runs>", str(sandbox.RUNS))) for spec in args.sources]
            print(report.write(specs=specs, out=args.out))
        elif args.sources:
            raise SystemExit("qeval report: --from needs --out <folder>, where results.md goes")
        elif args.out:
            raise SystemExit("qeval report: --out needs at least one --from agents=|scripted=|all=<run folder>")
        else:
            raise SystemExit(NO_RUN)
        return 0

    quadratic_bin = sandbox.build_quadratic()
    tools = args.tools.split(",")
    if args.command == "scripted":
        run = args.run or sandbox.RUNS / datetime.now().strftime("%Y%m%d-%H%M%S")
        run.mkdir(parents=True, exist_ok=True)
        print(f"run folder: {run}", flush=True)
        run_scripted(run, tools, args.rounds, args.repeats, quadratic_bin)
        print(report.write(run))
    elif args.command == "agents":
        from qeval import agents

        for name in tools:
            _save(args.run, "agents", name, agents.run_tool(ADAPTERS[name], args.run / name / "agents", quadratic_bin))
        print(report.write(args.run))
    elif args.command == "reanalyze":
        from qeval import agents

        for name in tools:
            _save(args.run, "agents", name, agents.reanalyze(ADAPTERS[name], args.run / name / "agents", quadratic_bin))
        print(report.write(args.run))
    found = sandbox.real_home_check()
    if any(found.values()):
        print("REAL HOME CHANGED:", json.dumps(found))
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
