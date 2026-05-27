import type { MemoryItem } from '$lib/types/memories';

export function hrefFor(row: MemoryItem): string {
  switch (row.kind) {
    case 'fact':
      return `/facts/${row.id}`;
    case 'knowledge':
      return `/knowledge/${row.id}`;
    case 'event':
      return `/events/${row.id}`;
  }
}

export function summaryFor(row: MemoryItem): string {
  switch (row.kind) {
    case 'fact':
      return `${row.type}: ${JSON.stringify(row.payload).slice(0, 120)}`;
    case 'knowledge':
      return row.text.slice(0, 160);
    case 'event':
      return `${row.category}: ${JSON.stringify(row.payload).slice(0, 120)}`;
  }
}
