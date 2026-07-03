# Third-party notices

SilicoLab is licensed under `MIT OR Apache-2.0` (see `LICENSE-MIT` and
`LICENSE-APACHE`). Most of its dependencies are permissively licensed
(MIT / Apache-2.0 / BSD / Zlib) and impose no notice obligation beyond what
their own published packages already carry.

This file records only the third-party components whose notices must travel
with a **binary** redistribution of SilicoLab — either because the license is
weak-copyleft (MPL-2.0), or because the component is vendored into this
repository and embedded into the binary, so its in-tree license file would not
otherwise reach a binary-only recipient.

A complete, machine-generated inventory of every dependency's license can be
produced with `cargo about` or `cargo license` from the workspace root.

---

## MPL-2.0 components

### xcx — exchange–correlation functionals for DFT

Copyright (c) 2026 Jiekang Tian and the xcx authors. Portions are derived from
libxc (<https://libxc.gitlab.io/>), Copyright (C) the libxc authors, including
M. A. L. Marques, X. Andrade, S. Lehtola, and M. Oliveira.

Pulled in transitively via `hartree`. SPDX: `(MIT OR Apache-2.0) AND MPL-2.0`.
xcx is licensed per file — its original code is `MIT OR Apache-2.0`, while
files derived from libxc remain under the Mozilla Public License, Version 2.0.
MPL-2.0 is file-level (weak) copyleft: it places no restriction on SilicoLab as
a whole, nor on your use of it; only modifications to the MPL-licensed source
files themselves must be released under the MPL-2.0.

SilicoLab links this code unmodified. The corresponding source — the
MPL-licensed files and the full license text — is published with the crate at
<https://crates.io/crates/xcx>. If you redistribute SilicoLab in binary form,
keep this notice available and, per MPL-2.0 § 3.2, ensure recipients can obtain
that source. The full MPL-2.0 text: <https://www.mozilla.org/MPL/2.0/>.

---

## Bundled data assets

These third-party data files are vendored into this repository and embedded
into the binary (via `include_str!`), so the copyright and permission notices
below must accompany binary redistributions.

### CHARMM36 force field

Copyright (c) 2025 mackerell-lab. Licensed under the MIT License. A pruned
subset is bundled under `assets/forcefields/charmm36.ff/`; the in-tree license
file is `assets/forcefields/charmm36.ff/LICENSE`.

The standard MIT permission notice below applies to every MIT-licensed asset in
this section:

> Permission is hereby granted, free of charge, to any person obtaining a copy
> of this software and associated documentation files (the "Software"), to deal
> in the Software without restriction, including without limitation the rights
> to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
> copies of the Software, and to permit persons to whom the Software is
> furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all
> copies or substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
> IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
> FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
> AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
> LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
> OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
> SOFTWARE.

## Liberation Sans (embedded font)

`assets/fonts/LiberationSans-Regular.ttf` (Liberation Fonts 2.1.5,
https://github.com/liberationfonts/liberation-fonts) is compiled into the
binary and embedded in exported charts (SVG text, PDF font subsets). Licensed
under the SIL Open Font License, Version 1.1; the full license text is vendored
at `assets/fonts/LiberationSans-LICENSE`.
