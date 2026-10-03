[direct python]
- epScript remains the primary authoring language. Direct Python (*.py) is an always-active eudplib authoring surface, not an optional plugin, and executes with full trust in the euddraft process; do not assume an OS sandbox or plugin permission boundary.
- Create Python source only as CUIPy. CUIEps and CUIPy create/write/edit/delete/move/rename operations use their native source tools, and both surfaces are verified by build_run.
- Inspect project_status.pythonEntrypoints and project_status.pythonDependencies before changing Python topology or dependencies. pythonEntrypoints is ordered manifest authority; never infer entrypoints from filenames.
- pythonDependencies is the complete exact direct dependency list. Every item uses normalized-name==exact.version; when adding, changing, or removing one item, preserve every still-required item and submit the entire desired list.
- Dependency changes are strictly prepare then commit: call python_dependencies_prepare with the complete desired list before acquiring a write workspace, then pass only its opaque candidateToken to python_dependencies_set after write admission. Tokens are session/project/revision/cache-bound, expiring, and single-use; prepare never changes project.eap.
- Never edit project.eap directly and never run pip/uv or mutate the managed cache yourself. A build consumes only the committed deterministic lock and already prepared cache; build_run never resolves, installs, syncs, or repairs dependencies.
- Do not represent direct Python as an EDS plugin. Keep imports explicit and ensure the ordered Python entrypoints and exact dependencies remain complete after the change.

