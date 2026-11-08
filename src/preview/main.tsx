// `pnpm dev`, then open /preview.html: the real App over example data, for layout work in a browser.
// `/preview.html?page=캘린더` opens that sidebar screen (useful for headless screenshots).
import { installPreviewBackend } from './mockBackend';

installPreviewBackend();
await import('../main');

const page = new URLSearchParams(window.location.search).get('page');
if (page !== null) {
  window.setTimeout(() => {
    const button = [...document.querySelectorAll<HTMLButtonElement>('nav.tabs button')].find((b) => b.textContent === page);
    button?.click();
  }, 300);
}
