# Moon

> Currently a work in progress!

Moon is a *m*onorepo *o*rganization, *o*rchestration, and *n*otification tool for JavaScript based
projects, written in Rust. Many of the concepts within Moon are heavily inspired from Bazel.

- [Documentation](./docs/README.md)

<!--
#### Tokens

- File groups
  - `@glob` - Returns the file group as a glob (typically as-is).
  - `@root` - Returns the file group, reduced down to the lowest possible directory.
  - `@dirs` - Returns the file group, reduced down to all possible directories.
  - `@files` - Returns the file group as a list of all possible files.
- Inputs & outputs
  - `@in` - Points to an index within a task's `inputs` list. This will be expanded to the
    underyling file path(s).
  - `@out` - Points to an index within a task's `outputs` list. This will be expanded to the
    underyling file path(s).
  - `@dep` - Points to an index within a task's `deps` list. This will be expanded to the underyling
    file path(s) of the task's output.
- Other
  - `@cache` - Returns an absolute file path to a location within the cache folder.
  - `@pid` - Returns the running project's ID as a fully-qualified ID from the workspace root.
-->
