import { describe, expect, it } from 'vitest';
import { isCheerMessage } from './messages';

describe('isCheerMessage', () => {
  it('accepts the shape the core emits', () => {
    expect(isCheerMessage({ kind: 'set_end', tone: 'warm', source: 'template', text: '잘했어요' })).toBe(true);
  });

  it('rejects unknown tones, empty text, and other shapes', () => {
    expect(isCheerMessage({ kind: 'set_end', tone: 'rude', source: 'template', text: 'x' })).toBe(false);
    expect(isCheerMessage({ kind: 'random', tone: 'warm', source: 'wiki', text: '' })).toBe(false);
    expect(isCheerMessage(null)).toBe(false);
    expect(isCheerMessage('text')).toBe(false);
  });
});
