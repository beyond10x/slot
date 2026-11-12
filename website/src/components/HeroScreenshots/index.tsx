import {useCallback, useEffect, useRef, useState, type KeyboardEvent, type ReactNode} from 'react';
import {useBaseUrlUtils} from '@docusaurus/useBaseUrl';
import styles from './styles.module.css';

export interface Shot {
  src: string;
  alt: string;
  caption: string;
}

const INTERVAL_MS = 5000;

function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(true);
  useEffect(() => {
    const query = window.matchMedia('(prefers-reduced-motion: reduce)');
    const update = () => setReduced(query.matches);
    update();
    query.addEventListener('change', update);
    return () => query.removeEventListener('change', update);
  }, []);
  return reduced;
}

/**
 * Screenshots of the web UI for the hero: one visible at a time, advancing every five seconds
 * unless the reader prefers reduced motion, hovers it or has focus inside it.
 */
export default function HeroScreenshots({shots, label}: {shots: Shot[]; label: string}): ReactNode {
  const [index, setIndex] = useState(0);
  const [held, setHeld] = useState(false);
  const reduced = useReducedMotion();
  const {withBaseUrl} = useBaseUrlUtils();
  const root = useRef<HTMLElement>(null);
  const count = shots.length;
  const go = useCallback((to: number) => setIndex(((to % count) + count) % count), [count]);

  useEffect(() => {
    if (reduced || held || count < 2) return undefined;
    const timer = window.setInterval(() => setIndex((i) => (i + 1) % count), INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [reduced, held, count]);

  const onKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === 'ArrowRight') {
      event.preventDefault();
      go(index + 1);
    } else if (event.key === 'ArrowLeft') {
      event.preventDefault();
      go(index - 1);
    }
  };

  const shot = shots[index];
  return (
    <section
      ref={root}
      className={styles.carousel}
      aria-roledescription="carousel"
      aria-label={label}
      onMouseEnter={() => setHeld(true)}
      onMouseLeave={() => setHeld(false)}
      onFocus={() => setHeld(true)}
      onBlur={(event) => {
        if (!root.current?.contains(event.relatedTarget as Node | null)) setHeld(false);
      }}
      onKeyDown={onKeyDown}>
      <div className={styles.frame}>
        <div className={styles.bar} aria-hidden="true">
          <span className={styles.lights}>
            <i />
            <i />
            <i />
          </span>
          <span className={styles.address}>127.0.0.1:7411</span>
        </div>
        <div className={styles.viewport} aria-live={held ? 'polite' : 'off'}>
          {shots.map((s, i) => (
            <figure
              key={s.src}
              className={i === index ? `${styles.slide} ${styles.current}` : styles.slide}
              role="group"
              aria-roledescription="slide"
              aria-label={`${i + 1} of ${count}: ${s.caption}`}
              aria-hidden={i !== index}>
              <a href={withBaseUrl(s.src)} target="_blank" rel="noopener" tabIndex={i === index ? 0 : -1}>
                <img src={withBaseUrl(s.src)} alt={s.alt} width={1600} height={1000} loading={i === 0 ? 'eager' : 'lazy'} decoding="async" />
                <span className={styles.srOnly}> (full size, opens in new tab)</span>
              </a>
            </figure>
          ))}
        </div>
      </div>
      <div className={styles.controls}>
        <button type="button" className={styles.arrow} onClick={() => go(index - 1)} aria-label="Previous screenshot">
          ←
        </button>
        <p className={styles.caption}>{shot.caption}</p>
        <div className={styles.dots}>
          {shots.map((s, i) => (
            <button
              key={s.src}
              type="button"
              className={i === index ? `${styles.dot} ${styles.dotCurrent}` : styles.dot}
              onClick={() => go(i)}
              aria-label={`Screenshot ${i + 1}: ${s.caption}`}
              aria-current={i === index ? 'true' : undefined}
            />
          ))}
        </div>
        <button type="button" className={styles.arrow} onClick={() => go(index + 1)} aria-label="Next screenshot">
          →
        </button>
      </div>
    </section>
  );
}
