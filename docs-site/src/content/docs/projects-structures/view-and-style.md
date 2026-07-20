---
title: Orient and style structures
description: Confirm atom scope, then adjust camera, representation, overlays, and visibility without assigning chemical meaning to display cues.
sidebar:
  order: 5
---

## Orient the current structure

Camera actions change only the view. They do not change atom coordinates or
structure content.

| Goal | Action | Visible result |
| --- | --- | --- |
| Rotate | Primary-button drag in the viewport | The structure rotates around the view center |
| Pan | Secondary- or middle-button drag | The whole structure moves across the viewport |
| Pan or zoom | Use a wheel, trackpad, pinch, or modified scroll over the viewport | The mapping depends on the input device; verify the resulting position or scale |
| Restore the camera | Press `R` for **Reset View** | The camera returns to a view containing the structure; styles and visibility remain unchanged |

Use the visible result rather than assuming that every trackpad or mouse maps
the same gesture identically.

## Confirm atom scope before styling

Open **Style** and read the scope message at the top.

| Current atom selection | Scope of Style actions | Panel message |
| --- | --- | --- |
| One or more atoms selected | Those atoms in the current entry | `Acting on N selected atom(s)` |
| No atoms selected | Every atom in the current entry | `No atoms selected - styles apply to all atoms` |

Highlighted entries and groups in the Entries sidebar do not affect Style
scope. To style the whole structure, clear atom selection, verify that viewport
and Sequence highlights disappear, and then confirm the all-atoms message.

## Choose one base representation

Under **Representation**, choose one mutually exclusive base representation.

| Option | Display change |
| --- | --- |
| **Ball-and-stick** | Atom balls and bond cylinders |
| **Stick** | Bond cylinders emphasize connectivity |
| **Wireframe** | Bonds appear as thin lines without atom balls |
| **Sphere (VdW)** | Atoms use larger renderer spheres based on element radii |
| **Dots** | Atoms appear as small dots |

After choosing a representation, check both the visible geometry and the atom
count in temporary status feedback. A representation changes rendering only; it
does not change elements, coordinates, bonds, or other structure data.

## Add Cartoon or Surface overlays

**Cartoon ribbon** and **Surface** are additive overlays. They can coexist and
do not replace the current base representation. Their switches use the same atom
scope as other Style actions.

- Cartoon needs compatible chain, residue, and backbone metadata. No visible
  ribbon does not by itself mean import failed or that the structure contains no
  polymer.
- Surface is renderer geometry generated from atom positions and display radii.
  It is not electron density, electrostatic potential, hydrophobicity, or
  another calculated chemical surface.

The first Surface enable can take time while geometry is generated. Surface
appearance controls under **Advanced > Surface** change fill or mesh style and
transparency for the current entry's display.

Turning an overlay off removes it from the current atom scope while leaving the
base representation visible.

## Show, hide, or isolate atoms

Visibility is independent of base representation and overlays.

| Action | Result |
| --- | --- |
| **Show** | Shows atoms in the current style scope |
| **Hide** | Hides atoms in the current style scope |
| **Isolate** | Shows only the current scope and hides other atoms in the current entry |

Hiding atoms does not delete them. To restore the whole structure after
Isolate, first clear atom selection, verify that no atoms remain selected, and
then choose **Show** or **Reset to default style**. Otherwise the action still
targets only the selected atoms and other atoms remain hidden.

**Reset to default style** resets representation, Cartoon and Surface overlay
state, and visibility for the current style scope. It does not reset the camera.
**Reset View** resets the camera only and does not restore styles or hidden atoms.

## Separate atom style from entry-wide display settings

These controls apply to the current entry's entire viewport even when only some
atoms are selected:

- **Background**, **Unit cell**, and **Atom labels**;
- **Light**, **Silhouettes**, and silhouette width;
- Cartoon cross-section and smoothing;
- Surface fill or mesh appearance and transparency.

The Surface switch selects which atoms participate in that overlay; the Surface
appearance controls change how the current entry's surface is drawn. Recheck
viewport state after switching entries because each entry can retain its own
view settings.

## Do not infer chemistry from display styling

Element colors, chain colors, selection tint, representation, visibility, and
renderer surfaces are display choices. Unless a separately labeled analysis
provides the evidence, do not interpret them as charge, electrostatic potential,
hydrophobicity, bond order confidence, accessibility, or another chemical
property.

## Related pages

- [Select entries, atoms, and sequence residues](../selection-and-sequence/)
- [Edit and export structures](../edit-and-export/)
- [Understand entries, groups, and action scope](../entries-and-groups/)
