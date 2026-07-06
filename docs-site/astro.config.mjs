import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  site: 'https://silicolab.github.io',
  base: '/silicolab',
  integrations: [
    starlight({
      title: 'SilicoLab',
      description:
        'Computational environment for chemistry, biology & materials research.',
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
      ],
    }),
  ],
});
