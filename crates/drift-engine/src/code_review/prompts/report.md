## Report

Call `report_findings` once with `level` `{level}` and at most {cap} findings, most severe first. Each needs `file` (relative to the workspace), `line`, `summary`, `short_summary`, `failure_scenario` and `category`{verdict}. When nothing survives, call it with an empty list. Then end with one or two sentences on the state of the change; do not list the findings again, since the user sees them as a card.
