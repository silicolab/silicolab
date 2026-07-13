# Assistant Composer Design QA

- Source visual truth: user-provided Cursor composer screenshots, originally attached from `C:\Users\jtian\Desktop\ScreenShot_2026-07-13_211845_574.png`, `...211829_517.png`, and `...211839_373.png`.
- Implementation screenshot: `C:\tmp\silicolab-assistant-composer-final.png`.
- Viewport: 1001 × 699 desktop window; Assistant right sidebar at its persisted narrow layout.
- State: first-run, API not configured, Assistant idle.

## Full-view comparison evidence

The final window capture shows the same two-tier composition as the reference: a full-width text area above a fixed bottom control row. The permission picker and model picker remain left-aligned; Send remains independently anchored to the right edge. The surrounding SilicoLab visual tokens are intentionally retained instead of cloning Cursor's colors.

## Focused composer comparison evidence

- Permission, model, and Send controls use the same 28px control height.
- The model label is reduced to `Sonnet 4.6`; permission is reduced to `Safe`.
- The model picker is capped at 132px and cannot consume the Send rectangle.
- The controls rectangle clips independently from the fixed Send/Stop rectangles.
- The user visually inspected the running final build and confirmed that it meets the requirement.

## Findings and comparison history

1. Earlier P1: the model picker could overlap Send because both depended on adaptive row allocation.
   - Fix: replaced shared-flow allocation with explicit right-anchored action rectangles and a separately clipped controls rectangle.
   - Post-fix evidence: `C:\tmp\silicolab-assistant-composer-final.png`; no overlap at the narrow sidebar width.
2. Earlier P2: model names remained verbose and permission icons rendered as question marks.
   - Fix: removed font-dependent permission icons, shortened permission labels, removed model qualifiers, and capped the collapsed model label.
   - Post-fix evidence: final capture shows `Safe` and `Sonnet 4.6` without missing glyphs.
3. Earlier P2: dropdowns and Send had visibly different heights.
   - Fix: set the bottom controls' interaction height to the same 28px used by Send/Stop.
   - Post-fix evidence: final capture shows a common baseline and height.

## Required fidelity surfaces

- Fonts and typography: existing SilicoLab UI font retained; no missing permission glyphs; labels remain readable and truncate safely.
- Spacing and layout rhythm: 4px picker gap, 6px action gap, 28px controls, and a fixed two-row 92px composer.
- Colors and visual tokens: SilicoLab semantic input, hairline, text, disabled, and accent colors retained.
- Image quality and asset fidelity: no raster assets are required; existing Phosphor Send/Stop icons remain crisp.
- Copy and content: compact permission/model labels in the collapsed row; complete permission descriptions remain available in the menu and tooltips.

## Implementation checklist

- [x] Text input cannot displace persistent actions.
- [x] Permission picker is available at point of action.
- [x] Per-conversation model picker is available at point of action.
- [x] Long model labels are shortened and capped.
- [x] Missing icon glyphs removed.
- [x] Control heights aligned.
- [x] Final running build visually accepted by the user.

final result: passed
