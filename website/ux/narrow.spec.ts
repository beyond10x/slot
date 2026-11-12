// On a phone the navbar links and the sidebar sit behind the menu button: on every merge, at 390x844
// in both themes, the navbar, the menu button and the search box are in view and not covered (DXF-007).
import { expect, test } from '@playwright/test';
import { isDocPage, sitemapPages, slug } from './pages.ts';
import { layout, openPage } from './probes.ts';
import { THEMES } from './viewports.ts';

for (const route of sitemapPages().filter((r) => r === '' || r === 'docs' || r === 'docs/getting-started')) {
  for (const theme of THEMES) {
    test(`narrow ${slug(route)} @ 390x844 ${theme}`, async ({ page }) => {
      await page.setViewportSize({ width: 390, height: 844 });
      await openPage(page, route, theme);
      const top = await layout(page, isDocPage(route));
      expect(top.hidden_always_visible, top.hidden_always_visible.join('\n')).toEqual([]);
      expect(top.doc_hscroll).toBe(0);
    });
  }
}
