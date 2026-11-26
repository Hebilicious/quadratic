"""Runs one step of .github/workflows/release.yml, as GitHub would: its `run` script with `bash -e`, in its
working directory, with the workflow's `env` as written, and the step's `env` taken from the variables of the same
name prefixed with STEP_."""
import os, subprocess, sys

import yaml

workflow, job, name = sys.argv[1:4]
document = yaml.safe_load(open(workflow))
step = next(s for s in document["jobs"][job]["steps"] if s.get("name") == name)
env = dict(os.environ)
env.update({key: str(value) for key, value in document.get("env", {}).items()})
env.update({key: os.environ.get("STEP_" + key, "") for key in step.get("env", {})})
sys.exit(subprocess.run(["bash", "-e", "-c", step["run"]], env=env, cwd=step.get("working-directory", ".")).returncode)
