# Python release readiness

The Python SDK (`transflow`) and supervised worker (`transflow_worker`) ship in
**one distribution named `transflow`**, sharing one version and wheel. The Rust CLI
is a separate native artifact. Publishing only the wheel does not install a
complete native Transflow application.

## Current position

Local wheel building, isolated installation and matched-worker qualification are
implemented. The package still has development version `0.0.0.dev0` and the
`Private :: Do Not Upload` classifier in
[`python/pyproject.toml`](../../python/pyproject.toml). Keep that upload guard until
an explicit release is prepared and approved.

A public PyPI JSON lookup for `transflow` returned HTTP 404 on 2026-10-17. That is
an observation, not a namespace reservation, ownership check or promise that an
upload will be accepted. Confirm the intended PyPI account/project ownership at
release time. No account or package has been registered by this work.

## When to propose the first public preview

Reassess after the prioritized editor and retention work is usable and tested.
A limited alpha can be proposed without claiming the full specification is complete,
provided its supported features and platform limits are explicit. Before proposing
an upload, prepare and review all of the following:

- A chosen release version and matching CLI/SDK/worker compatibility metadata.
- Clean installation and an end-to-end build from the exact wheel and native CLI
  on each advertised platform, without either source checkout or private spec.
- Passing applicable CI, package-content/privacy/license checks and runtime
  qualification for the supported Python 3.14 environment.
- Public package metadata, project URLs, license files and a PyPI-readable package
  description, including installation of the native CLI and its web assets.
- A reproducible release procedure, immutable artifacts and checksums, supported
  dependency setup, and a documented recovery path for incompatible versions.
- Owner-confirmed namespace access and an explicitly approved upload target/version.

Record any missing item as a release blocker. The current development guard and
version must not be removed merely to make an upload command succeed.

## Publication sequence

1. Build and inspect the proposed wheel and source distribution. Test their
   installation separately from editable/source installs.
2. After owner approval, rehearse publication and clean installation with TestPyPI,
   using explicit dependency sources; the test index is separate from production.
3. Prefer a narrowly configured Trusted Publisher for the approved release workflow.
   Keep publishing permissions out of ordinary build/test jobs.
4. Publish the reviewed production version only after the rehearsal and owner
   approval. Verify the installed version and record its artifacts and provenance.

## PyPI and GitHub administration follow-up

Requested by the owner on 2026-10-26. Track this setup alongside T116 before the
first release; account administration and the publishing workflow remain pending.

- Confirm the owning PyPI account, verified email, two-factor authentication and
  access to the intended `transflow` project name. Check TestPyPI separately.
- Review the release workflow before registering its Trusted Publisher. The
  proposed identity is GitHub owner `jmsmistral`, repository `transflow`, workflow
  `publish-python.yml`, production environment `pypi` and test environment
  `testpypi`. These are proposed settings, not existing configured publishers.
- In PyPI account settings, add a pending GitHub Actions publisher for `transflow`
  if the project does not exist; otherwise configure its project publisher. The
  workflow filename and environment must match the eventual GitHub job exactly.
  A pending publisher does not reserve the project name.
- Configure the corresponding GitHub environments, release restrictions and
  required approvals where available. Set up a separate TestPyPI publisher.
- Implement the reviewed workflow with build/test jobs separated from publishing.
  Give only the publishing job `id-token: write` for OIDC Trusted Publishing,
  using short-lived credentials rather than a stored PyPI upload token. Ordinary
  pushes must not publish a package.
- Rehearse the approved artifacts on TestPyPI before the first approved production
  upload. Record account setup, workflow identity and installation evidence here.

Administrative setup does not close the release-readiness checks above. The
development version and private upload classifier remain until release approval.
See PyPI's [publisher setup](https://docs.pypi.org/trusted-publishers/adding-a-publisher/)
and [pending project publisher](https://docs.pypi.org/trusted-publishers/creating-a-project-through-oidc/)
instructions.

This is a readiness checklist, not an enabled publishing pipeline or authorization
to upload. Official references: [PyPA packaging tutorial](https://packaging.python.org/en/latest/tutorials/packaging-projects/),
[TestPyPI guide](https://packaging.python.org/en/latest/guides/using-testpypi/) and
[PyPI Trusted Publishers](https://docs.pypi.org/trusted-publishers/).
