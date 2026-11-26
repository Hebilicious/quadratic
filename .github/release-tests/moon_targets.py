"""Prints each `moon run` target of the workflows that moon would not run in CI, one per line: a target whose task
does not exist, or has `runInCI: false`. For such a target `moon run` answers "No tasks found" in a workflow and
fails, although it runs on a person's machine. Argument: the repository's root."""
import pathlib, re, sys

import yaml

root = pathlib.Path(sys.argv[1])
tasks = {}
for file in root.glob("apps/**/moon.yml"):
    if "node_modules" in file.parts:
        continue
    project = yaml.safe_load(file.read_text())
    for name, task in (project.get("tasks") or {}).items():
        tasks[f"{project['id']}:{name}"] = task or {}

for workflow in sorted((root / ".github/workflows").glob("*.yml")):
    for job in yaml.safe_load(workflow.read_text())["jobs"].values():
        for step in job.get("steps", []):
            text = step.get("run", "").replace("\\\n", " ")
            for line in text.splitlines():
                found = re.search(r"\bmoon run\b(.*)", line)
                if not found:
                    continue
                for target in re.findall(r"[\w-]+:[\w-]+", found.group(1)):
                    task = tasks.get(target)
                    if task is None:
                        print(f"{workflow.name}: {target} is not a task")
                    elif (task.get("options") or {}).get("runInCI") is False:
                        print(f"{workflow.name}: {target} has runInCI: false")
