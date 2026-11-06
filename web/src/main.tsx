import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { mockApi, sessionTokens, WebApiClient, type WebApi } from './api';
import { loadConfig } from './config';
import './styles.css';

const root = document.getElementById('root');
if (!root) {
  throw new Error('root element missing from index.html');
}
const mount = createRoot(root);

function today(): string {
  return new Date().toISOString().slice(0, 10);
}

loadConfig(window.fetch.bind(window)).then((config) => {
  if (config === null) {
    mount.render(<main className="page"><p className="error">config.json을 읽지 못했거나 값이 올바르지 않습니다.</p></main>);
    return;
  }
  const api: WebApi = config.mode === 'mock' ? mockApi(today()) : new WebApiClient(config, window.fetch.bind(window), sessionTokens(window.sessionStorage));
  mount.render(
    <StrictMode>
      <App api={api} />
    </StrictMode>,
  );
}).catch(() => undefined);
