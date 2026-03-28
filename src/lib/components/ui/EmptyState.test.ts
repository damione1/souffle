import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import EmptyState from './EmptyState.svelte';

describe('EmptyState', () => {
  it('renders message text', () => {
    render(EmptyState, { props: { message: 'No meetings yet' } });

    expect(screen.getByText('No meetings yet')).toBeTruthy();
  });

  it('renders title when provided', () => {
    render(EmptyState, { props: { title: 'Empty', message: 'Nothing here' } });

    expect(screen.getByText('Empty')).toBeTruthy();
    expect(screen.getByText('Nothing here')).toBeTruthy();
  });
});
