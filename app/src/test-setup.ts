import '@testing-library/jest-dom/vitest';
import { clearMocks } from '@tauri-apps/api/mocks';
import { afterEach } from 'vitest';

afterEach(() => {
  clearMocks();
  // A test may have faked the page visibility.
  Reflect.deleteProperty(document, 'visibilityState');
});
