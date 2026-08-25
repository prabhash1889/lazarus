import { useTheme } from '../theme/ThemeProvider';
import { Button } from './Button';

export function ThemeToggle() {
  const { resolved, toggle } = useTheme();
  return (
    <Button variant="ghost" className="theme-toggle" onClick={toggle} aria-label="Toggle color theme">
      {resolved === 'dark' ? 'Light' : 'Dark'}
    </Button>
  );
}
