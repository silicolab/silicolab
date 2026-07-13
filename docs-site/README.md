# SilicoLab documentation site

This directory is the Astro/Starlight application for the public, localized
end-user manual. Contributor and maintainer documentation belongs in
[`../docs/`](../docs/README.md), not in the site content tree.

## Development

Use Node.js 22, matching the documentation workflow:

```sh
npm ci
npm run dev
```

Run `npm run build` before submitting changes. The build validates Starlight
content and internal links. Cloudflare builds and deploys the site from this
directory after changes reach `main`; `.github/workflows/docs.yml` performs the
pull-request validation build.

## Content and assets

- Put English user documentation under `src/content/docs/` and Simplified
  Chinese counterparts under `src/content/docs/zh/`.
- Update both maintained locales when behavior or instructions change.
- Link to canonical repository policies instead of copying them into the site.
- Do not edit generated files under `src/assets/` or `public/`. The
  `predev`/`prebuild` hooks run `scripts/sync-assets.mjs`, which copies canonical
  branding, screenshot, and icon assets from the repository root.

If a new document is primarily for people modifying or releasing SilicoLab,
put it in `../docs/` or a named root policy file. If it helps users install,
configure, operate, or troubleshoot the product, put it in this site.
