import type {Config} from '@docusaurus/types';
import type * as Preset from '@docusaurus/preset-classic';
import {withProductSite} from '@beyond10x/docs-system/product-site';

const config: Config = {
  title: 'slot',
  tagline: 'One shared pool of build slots for every build on a machine.',

  future: {
    v4: true,
  },

  url: 'https://beyond10x.github.io',
  baseUrl: '/slot/',

  organizationName: 'beyond10x',
  projectName: 'slot',
  deploymentBranch: 'gh-pages',
  trailingSlash: false,

  onBrokenLinks: 'throw',
  onBrokenAnchors: 'throw',

  markdown: {
    format: 'detect',
    hooks: {
      onBrokenMarkdownLinks: 'throw',
    },
  },

  i18n: {
    defaultLocale: 'en',
    locales: ['en'],
  },

  presets: [
    [
      'classic',
      {
        docs: {
          sidebarPath: './sidebars.ts',
          routeBasePath: 'docs',
          editUrl: 'https://github.com/beyond10x/slot/tree/main/website/',
        },
        blog: false,
      } satisfies Preset.Options,
    ],
  ],

  themeConfig: {
    navbar: {
      title: 'slot',
      items: [
        {to: '/docs/', label: 'Documentation', position: 'left'},
        {to: '/docs/getting-started', label: 'Get started', position: 'left'},
        {to: '/docs/how-it-works', label: 'How it works', position: 'left'},
        {to: '/docs/agent-integration', label: 'Agents', position: 'left'},
        {to: '/docs/reference/cli', label: 'CLI', position: 'left'},
        {href: 'https://github.com/beyond10x/slot', label: 'GitHub', position: 'right'},
      ],
    },
    footer: {
      links: [
        {
          title: 'Documentation',
          items: [
            {label: 'Overview', to: '/docs/'},
            {label: 'Getting started', to: '/docs/getting-started'},
            {label: 'How it works', to: '/docs/how-it-works'},
            {label: 'Toolchains', to: '/docs/toolchains'},
            {label: 'Remote builds', to: '/docs/remote-builds'},
            {label: 'Agent integration', to: '/docs/agent-integration'},
            {label: 'CLI reference', to: '/docs/reference/cli'},
            {label: 'Environment variables', to: '/docs/reference/environment'},
          ],
        },
        {
          title: 'Project',
          items: [
            {label: 'Source', href: 'https://github.com/beyond10x/slot'},
            {label: 'Releases', href: 'https://github.com/beyond10x/slot/releases'},
            {label: 'Changelog', href: 'https://github.com/beyond10x/slot/blob/main/CHANGELOG.md'},
            {label: 'Design', href: 'https://github.com/beyond10x/slot/blob/main/docs/design.md'},
          ],
        },
      ],
      copyright: 'A beyond10x project. slot · Apache-2.0.',
    },
  } satisfies Preset.ThemeConfig,
};

export default withProductSite(config, {landing: './product.json', mark: 'sl'});
