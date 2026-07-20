import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightLinksValidator from 'starlight-links-validator';

export default defineConfig({
  site: 'https://docs.silicolab.nmrtist.space',
  integrations: [
    starlight({
      title: 'SilicoLab',
      description:
        'Computational environment for chemistry, biology & materials research.',
      // Fail the build on broken internal links. Relative links are allowed
      // (pages use them so they survive) and skipped by the check; absolute
      // links are resolution-checked, so a broken one — the classic mistake,
      // and the bug that shipped in the first docs PR — fails the build.
      plugins: [starlightLinksValidator({ errorOnRelativeLinks: false })],
      social: [
        {
          icon: 'github',
          label: 'GitHub',
          href: 'https://github.com/silicolab/silicolab',
        },
      ],
      defaultLocale: 'root',
      locales: {
        root: { label: 'English', lang: 'en' },
        zh: { label: '简体中文', lang: 'zh-CN' },
      },
      sidebar: [
        {
          label: 'Getting Started',
          translations: { 'zh-CN': '快速开始' },
          autogenerate: { directory: 'getting-started' },
        },
        {
          label: 'Projects & Structures',
          translations: { 'zh-CN': '项目与结构' },
          autogenerate: { directory: 'projects-structures' },
        },
        {
          label: 'Build & Prepare',
          translations: { 'zh-CN': '构建与准备' },
          autogenerate: { directory: 'build-prepare' },
        },
      ],
      logo: {
        light: './src/assets/wordmark-light.svg',
        dark: './src/assets/wordmark-dark.svg',
        replacesTitle: true,
      },
      favicon: '/favicon.png',
      customCss: ['./src/styles/custom.css'],
    }),
  ],
});
