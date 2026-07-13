# Repository documentation

This directory contains documentation for contributors and maintainers working
on the SilicoLab repository. The public end-user manual and its website live in
[`docs-site/`](../docs-site/README.md).

## Where documentation belongs

| Content | Location |
| --- | --- |
| Project overview and primary entry points | Root `README.md` |
| Contribution, architecture, release, and security policy | Named root documents |
| Detailed implementation and maintainer guides | `docs/` |
| Public installation, configuration, workflows, and troubleshooting | `docs-site/src/content/docs/` |
| Rust API contracts | Rust doc comments next to the API |
| Shared screenshots used by root documents and the site | `docs/images/` |

Keep a fact in one canonical place and link to it elsewhere. Do not copy a
maintainer guide into the public manual or duplicate user instructions in this
directory. English and translated site pages are intentional counterparts;
changes to user-visible behavior should update every maintained locale in the
same pull request.

Repository documentation is validated by `cargo xtask check-docs`, which is
also part of `cargo pr-check`. The site has a separate Node build because its
MDX, localization, navigation, and generated assets require Astro/Starlight.

## Current guides

- [Adding a feature](adding-a-feature.md)
- [Developing remote execution](developing-remote-execution.md)
- [Testing external engines](testing-external-engines.md)
