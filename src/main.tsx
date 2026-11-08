import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { QuickChatScreen } from './pages/QuickChatScreen';
import '@fontsource/ibm-plex-sans-kr/korean-400.css';
import '@fontsource/ibm-plex-sans-kr/korean-500.css';
import '@fontsource/ibm-plex-sans-kr/korean-600.css';
import '@fontsource/ibm-plex-sans-kr/korean-700.css';
import '@fontsource/ibm-plex-sans-kr/latin-400.css';
import '@fontsource/ibm-plex-sans-kr/latin-600.css';
import '@fontsource/ibm-plex-mono/500.css';
import '@fontsource/ibm-plex-mono/600.css';
import './styles.css';

const root = document.getElementById('root');
if (!root) {
  throw new Error('root element missing from index.html');
}

createRoot(root).render(
  <StrictMode>
    {window.location.hash === '#chat-quick' ? <QuickChatScreen /> : <App />}
  </StrictMode>,
);
