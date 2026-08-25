import { createRoot } from 'react-dom/client';
import { AppRoot } from './app/AppRoot';
import './index.css';

const container = document.getElementById('root');
if (!container) {
  throw new Error('root element not found');
}

createRoot(container).render(<AppRoot />);
