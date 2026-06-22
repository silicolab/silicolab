# Third-party notices

SilicoLab itself is licensed under MIT OR Apache-2.0. Most of its dependencies
are likewise permissively licensed (MIT / Apache-2.0 / BSD / Zlib), which require
no further notice here. This file lists the third-party components whose licenses
carry an obligation **beyond** MIT/Apache-2.0 when SilicoLab is redistributed in
binary form.

## MPL-2.0

- **`xcx`** (exchange–correlation functionals for DFT), pulled in transitively
  via `hartree`. Licensed `(MIT OR Apache-2.0) AND MPL-2.0`: it contains a
  Mozilla Public License 2.0 component (libxc-interoperable functional code).

  The MPL-2.0 is a file-level copyleft. SilicoLab links this code unmodified; its
  source — including the MPL-2.0 files and the full license text — is published
  with the crate at <https://crates.io/crates/xcx> (and its repository). If you
  redistribute SilicoLab binaries, keep this notice and that source pointer
  available; if you modify the MPL-2.0 files, you must release those changes
  under MPL-2.0.

A full machine-generated license inventory of every dependency can be produced
with `cargo about` or `cargo license` from the workspace root.
