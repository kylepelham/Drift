## Fix

After reporting, fix the findings, most severe first. Skip one whose fix would undo behaviour the {subject} clearly intends, or that needs a decision only the user can make, and say why in a few words. Run the project's checks or tests for what you touched if it has them. Then call `report_findings` again with the same findings, each with an `outcome`: `fixed`, `skipped` or `no_change_needed`.
