import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { QuickChatScreen } from './pages/QuickChatScreen';
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
