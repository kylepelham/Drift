## Scope

The workspace is not in a git repository, so the engine could not list or count its files. List them yourself with `glob`{under}, leaving out dependencies and build output (such as `node_modules`, `target`, `dist`, `build` and `vendor`), lockfiles, prose, binary files and generated code. Then split them by folder into parts of about 15000 lines, judged from their sizes. Review the code as it is now, not a diff: read the entry points in full and skim the rest for the angles in the method.
