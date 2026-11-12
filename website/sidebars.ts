import type {SidebarsConfig} from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  docsSidebar: [
    {
      type: 'category',
      label: 'Start here',
      collapsed: false,
      items: ['index', 'getting-started'],
    },
    {
      type: 'category',
      label: 'Concepts',
      collapsed: false,
      items: ['how-it-works', 'toolchains'],
    },
    {
      type: 'category',
      label: 'Guides',
      collapsed: false,
      items: ['remote-builds', 'agent-integration'],
    },
    {
      type: 'category',
      label: 'Reference',
      collapsed: false,
      items: ['reference/cli', 'reference/environment'],
    },
  ],
};

export default sidebars;
