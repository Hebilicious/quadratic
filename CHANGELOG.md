# Changelog

## [0.3.0](https://github.com/Hebilicious/quadratique/compare/v0.2.0...v0.3.0) (2026-11-26)


### Features

* an agent moves an issue assigned to the human on the human's words, at high confidence ([#44](https://github.com/Hebilicious/quadratique/issues/44)) ([cc8051e](https://github.com/Hebilicious/quadratique/commit/cc8051ee581e3b357e0db6a8355a39279d606504))
* quadratique ui pairs the web app with a token the page saves and sends on every request ([#43](https://github.com/Hebilicious/quadratique/issues/43)) ([1196fce](https://github.com/Hebilicious/quadratique/commit/1196fce59d8b86c7d7ecfda7b18b41502a8f4480))
* the web app keeps its address across restarts by default ([#41](https://github.com/Hebilicious/quadratique/issues/41)) ([a90d71e](https://github.com/Hebilicious/quadratique/commit/a90d71e46933b7c6af671044bf647ada39576b04))

## [0.2.0](https://github.com/Hebilicious/quadratique/compare/v0.1.0...v0.2.0) (2026-11-26)


### ⚠ BREAKING CHANGES

* the command `close` and the capabilities `issue.close` and `issues.close` are now `finish`, `issue.finish` and `issues.finish`. The old names still answer on the command line and over HTTP until the release after 0.2, with one line on standard error that names `finish`. MCP offers only `issue.finish`. The refusal of `issues.finish` is `NotFinished`.
* a Quadratique data folder that a group or other accounts may use, that an ACL entry shares, that sits under a folder another account can change, or whose socket path is longer than the system allows is refused with exit status 5 instead of being made private. Run what the message names: `chmod 700 <folder>` for its mode; `setfacl -b <folder>` or `setfacl -k <folder>` (Linux) or `chmod -N <folder>` (macOS) for an ACL entry; `chmod go-w <parent>`, or another `QUADRATIQUE_HOME`, for a parent that another account can change; a shorter `QUADRATIQUE_HOME` for a socket path that is too long. Then restart the service if one runs the server.

### Features

* `finish` is the verb that hands finished work to review ([#42](https://github.com/Hebilicious/quadratique/issues/42)) ([9104b31](https://github.com/Hebilicious/quadratique/commit/9104b31503336475930fc06f64f8711e73def099))
* `quadratique init` updates a skill that an earlier build wrote ([#17](https://github.com/Hebilicious/quadratique/issues/17)) ([9928721](https://github.com/Hebilicious/quadratique/commit/99287218cfd8802ceaeb0d698c57571c7033dfeb))
* any agent may release a claim in review, and a session acts on its claims from any worktree ([#20](https://github.com/Hebilicious/quadratique/issues/20)) ([f0b3930](https://github.com/Hebilicious/quadratique/commit/f0b3930f199b5a3a32abcff5e71776eceaf3bfb5))
* quadratique upgrade installs the latest release and reports each repository's skill ([#26](https://github.com/Hebilicious/quadratique/issues/26)) ([5734d07](https://github.com/Hebilicious/quadratique/commit/5734d0707e9c2d00d08c74bbc7181f7bb2cd8add))
* saved views live in the server's store, not in the browser's storage ([#35](https://github.com/Hebilicious/quadratique/issues/35)) ([5f74460](https://github.com/Hebilicious/quadratique/commit/5f74460755872798fa17c5fa306ec8f1f86c1734))
* stop and take-over are requests on the server's socket, addressed to one generation ([#36](https://github.com/Hebilicious/quadratique/issues/36)) ([e2dc702](https://github.com/Hebilicious/quadratique/commit/e2dc7023bb9e46e22081b1efd71066cf76f97429))
* the command line stops a server of release 0.1 only with proof from /proc ([#38](https://github.com/Hebilicious/quadratique/issues/38)) ([d56e5cd](https://github.com/Hebilicious/quadratique/commit/d56e5cdd0953e8df4e6e03d298827bb980cd2751))
* the server also listens on a Unix socket in its data folder, and server status uses it ([#34](https://github.com/Hebilicious/quadratique/issues/34)) ([09e4894](https://github.com/Hebilicious/quadratique/commit/09e4894316dc189bbd3734251979f9f1fd98f352))


### Bug Fixes

* a dropped server stops listening before it frees its home ([#27](https://github.com/Hebilicious/quadratique/issues/27)) ([b6a840e](https://github.com/Hebilicious/quadratique/commit/b6a840ef04216f8804d342419f9c7c2e0de7edb0))
* a refused listener shutdown is logged, and its test needs no pidfd ([#33](https://github.com/Hebilicious/quadratique/issues/33)) ([64e2129](https://github.com/Hebilicious/quadratique/commit/64e2129f1890c89708878a0d651a2ffdb0b20b97))
* Quadratique builds and tests on macOS and Windows, and refuses to serve there without a caller check ([#21](https://github.com/Hebilicious/quadratique/issues/21)) ([5076224](https://github.com/Hebilicious/quadratique/commit/50762244511e85854a9dcb31f4b7358cccab159f))
* quadratique upgrade stops the home's server by the generation it saw, over the socket ([#39](https://github.com/Hebilicious/quadratique/issues/39)) ([28fb7f9](https://github.com/Hebilicious/quadratique/commit/28fb7f91d37a7d518a0ddfc3bf816c07ac4f779f))
* SQLite without allocation accounting, faster findings.import, and server tests far from their hang guard ([#24](https://github.com/Hebilicious/quadratique/issues/24)) ([dc9ea43](https://github.com/Hebilicious/quadratique/commit/dc9ea43af96ceda74edde6398bf4d91a846364c4))

## [0.1.0](https://github.com/Hebilicious/quadratique/compare/v0.1.0...v0.1.0) (2026-11-25)


### ⚠ BREAKING CHANGES

* rename to quadratique ([#9](https://github.com/Hebilicious/quadratique/issues/9))

### Features

* `issue list --claim mine` lists every claim of the caller's session ([#12](https://github.com/Hebilicious/quadratique/issues/12)) ([57e4140](https://github.com/Hebilicious/quadratique/commit/57e414044b81fd417fe46048e95f8d19ce44b334))
* `quadratique close` closes several issues with the same evidence ([#13](https://github.com/Hebilicious/quadratique/issues/13)) ([acbb941](https://github.com/Hebilicious/quadratique/commit/acbb9415a19a58f38ffaa0fa75a099e286ce74a0))
* an issue shows its branch, pull request and landed commits from local git ([#16](https://github.com/Hebilicious/quadratique/issues/16)) ([fb067cf](https://github.com/Hebilicious/quadratique/commit/fb067cffcdb6e59ba55443425e4111ab352829a9))
* the skill says what a lead does with a claim whose holder ended ([#14](https://github.com/Hebilicious/quadratique/issues/14)) ([f4e2319](https://github.com/Hebilicious/quadratique/commit/f4e231913c5bdfc12906891042884575dd4e717b))


### Bug Fixes

* two server process tests assert what the server promises ([#15](https://github.com/Hebilicious/quadratique/issues/15)) ([19bca6d](https://github.com/Hebilicious/quadratique/commit/19bca6d7084b33d84231b741f894e57dcbde03be))


### Miscellaneous Chores

* rename to quadratique ([#9](https://github.com/Hebilicious/quadratique/issues/9)) ([3b8a755](https://github.com/Hebilicious/quadratique/commit/3b8a755e7a25473f17e3bbce818dd5f665ec071d))
