import { defineConfig } from 'vitepress'

export default defineConfig({
  title: 'qbrsh',
  description: 'A fast, keyboard-driven web browser in Rust',
  cleanUrls: true,
  // The theme is dark by design; forcing dark also lets VitePress paint the
  // dark background before hydration, avoiding a white flash on load.
  appearance: 'force-dark',
  themeConfig: {
    nav: [
      { text: 'Guide', link: '/guide/getting-started' },
      { text: 'Reference', link: '/reference/commands' },
      { text: 'GitHub', link: 'https://github.com/QaidVoid/qbrsh' },
    ],
    sidebar: {
      '/guide/': [
        {
          text: 'Introduction',
          items: [
            { text: 'What is qbrsh?', link: '/guide/what-is-qbrsh' },
            { text: 'Getting Started', link: '/guide/getting-started' },
          ],
        },
        {
          text: 'Using qbrsh',
          items: [
            { text: 'Keybindings', link: '/guide/keybindings' },
            { text: 'Configuration', link: '/guide/configuration' },
            { text: 'Ad Blocking', link: '/guide/ad-blocking' },
            { text: 'Permissions', link: '/guide/permissions' },
          ],
        },
        {
          text: 'Going Further',
          items: [
            { text: 'Plugins', link: '/guide/plugins' },
            { text: 'Automation (IPC)', link: '/guide/automation' },
            { text: 'Architecture', link: '/guide/architecture' },
          ],
        },
      ],
      '/reference/': [
        {
          text: 'Reference',
          items: [
            { text: 'Commands', link: '/reference/commands' },
            { text: 'Plugin API', link: '/reference/plugin-api' },
          ],
        },
      ],
    },
    socialLinks: [{ icon: 'github', link: 'https://github.com/QaidVoid/qbrsh' }],
    outline: { level: [2, 3] },
  },
})
