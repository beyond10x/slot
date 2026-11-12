# D-01 decision: does the docs site get its own search box in the navbar?

Finding: DXF-001. Target metric: `always_visible.hidden` (search), now 44 of 44 gating runs, expected 0.
Yes: `@easyops-cn/docusaurus-search-local` 0.55.3 indexes the docs at build time and puts a search
box in the navbar; nothing leaves the reader's browser. Shot: `shots/docs@1366x768-light.png` of D-02.
No: the navbar stays as it is; a reader finds pages by the navbar, the sidebar and the landing page,
and the organization search at beyond10x.github.io/search stays one site away.
Default if unanswered by the next round: yes, because every reader task that names no navbar link
needs a steer without it, and another beyond10x site already ships the same plugin.
Answer: yes (default), 2026-11-12
