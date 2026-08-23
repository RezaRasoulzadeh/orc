export function formatTimestamp(value: string | Date | null | undefined): string {
  if (!value) return '—'
  const date = value instanceof Date ? value : new Date(/[zZ]|[+-]\d{2}:?\d{2}$/.test(value) ? value : `${value}Z`)
  if (Number.isNaN(date.getTime())) return 'invalid timestamp'
  return new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' }).format(date)
}

export function formatDuration(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds)) return '—'
  if (seconds < 60) return `${Math.round(seconds)}s`
  const minutes = Math.floor(seconds / 60)
  return minutes < 60 ? `${minutes}m ${Math.round(seconds % 60)}s` : `${Math.floor(minutes / 60)}h ${minutes % 60}m`
}

export function formatCount(value: number | null | undefined): string { return value == null ? '—' : new Intl.NumberFormat().format(value) }

export function formatPercent(value: number | null | undefined): string { return value == null ? '—' : `${Math.round(value)}%` }
