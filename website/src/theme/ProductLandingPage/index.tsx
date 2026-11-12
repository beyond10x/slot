/**
 * The docs-system landing page with screenshots of the web UI in the hero.
 *
 * `@beyond10x/docs-system` renders `product.art` (a terminal, a graph, a case card or a code pair)
 * beside the promise and has no art kind for images. This page keeps everything else from the
 * docs-system (hero copy, KPI row, sections and their anchors) and puts `HeroScreenshots` in the
 * hero instead. Sections of a kind this page does not know render nothing, as in the docs-system.
 */
import type {ComponentProps, ReactNode} from 'react';
import Layout from '@theme/Layout';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';
import {usePluginData} from '@docusaurus/useGlobalData';
import {
  FeatureGrid,
  Flow,
  ProductHero,
  ProductSection,
  RelatedTools,
  StatusStrip,
  Terminal,
} from '@beyond10x/docs-system/product';
import {
  landingDescription,
  landingKpis,
  landingTitle,
  type ProductLandingData,
  type ProductLandingSection,
} from '@beyond10x/docs-system/product-data';
import HeroScreenshots, {type Shot} from '@site/src/components/HeroScreenshots';

const SHOTS: Shot[] = [
  {
    src: '/img/ui-host-census.png',
    alt: 'slot web UI: a stacked chart of build cores on the host by tool family, run by slot or not; CPU, memory and IO pressure; the controller recommendation against the pool size; build cores per agent session; and a timeline of jobs with their wait for a slot.',
    caption: 'Every build on the host, by tool family and session, with pressure and the timeline.',
  },
  {
    src: '/img/ui-queue-live-units.png',
    alt: 'slot web UI: two jobs queued for slots in order, and running jobs with the crates each one compiles now, its cores and memory.',
    caption: 'The queue in arrival order, and what each running job compiles now.',
  },
  {
    src: '/img/ui-job-output.png',
    alt: 'slot web UI: the compile error of a failed cargo build in a side panel, beside the queued, running and finished jobs with exit code, wait, duration and CPU seconds.',
    caption: 'Output of any job, live or finished, beside its exit code, wait and CPU seconds.',
  },
];

const SECTION_IDS: Record<string, string> = {
  features: 'what',
  flow: 'how',
  status: 'status',
  related: 'related',
  terminal: 'try',
  'protocol-graph': 'protocol',
  'domain-graph': 'domain',
};

function sectionId(sections: ProductLandingSection[], index: number): string {
  const base = SECTION_IDS[sections[index].kind];
  const earlier = sections.slice(0, index).filter((s) => s.kind === sections[index].kind).length;
  return earlier ? `${base}-${earlier + 1}` : base;
}

type ProductId = ComponentProps<typeof RelatedTools>['current'];

function Section({section, current}: {section: ProductLandingSection; current?: ProductId}): ReactNode {
  switch (section.kind) {
    case 'features':
      return <FeatureGrid items={section.items} />;
    case 'flow':
      return <Flow steps={section.steps} />;
    case 'status':
      return typeof section.items === 'string' ? null : (
        <StatusStrip items={section.items} asOf={section.asOf} source={section.source} />
      );
    case 'related':
      return <RelatedTools tools={section.tools} current={current} />;
    case 'terminal':
      return typeof section.session === 'string' ? null : <Terminal session={section.session} />;
    default:
      return null;
  }
}

export default function ProductLandingPage({landing}: {landing: ProductLandingData}): ReactNode {
  const {siteConfig} = useDocusaurusContext();
  const current = (usePluginData('b10x-product-site') as {product?: ProductId} | undefined)?.product;
  const {product, sections} = landing;
  return (
    <Layout
      title={landingTitle(product, siteConfig.title)}
      description={landingDescription(product)}
      wrapperClassName="b10x-product-page">
      <main id="b10x-product-main">
        <div className="b10x-product">
          <ProductHero
            eyebrow={product.eyebrow}
            promise={product.promise}
            lede={product.lede}
            actions={product.actions}
            meta={product.meta}
            aside={<HeroScreenshots shots={SHOTS} label="Screenshots of the slot web UI" />}
            kpis={landingKpis(landing)}
          />
          {sections.map((section, index) => (
            <ProductSection
              key={`${section.kind}-${index}`}
              id={sectionId(sections, index)}
              number={index + 1}
              eyebrow={section.eyebrow}
              title={section.title}
              lede={section.lede}
              kind={section.kind}>
              <Section section={section} current={current} />
            </ProductSection>
          ))}
        </div>
      </main>
    </Layout>
  );
}
