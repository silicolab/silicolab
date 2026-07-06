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
      sidebar: [
        {
          label: 'Getting Started',
          autogenerate: { directory: 'getting-started' },
        },
      ],
    }),
  ],
});
