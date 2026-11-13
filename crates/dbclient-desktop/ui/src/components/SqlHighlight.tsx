// Highlight layer of the SQL editor: a <pre> under a transparent textarea,
// same font, padding and no wrapping, so each coloured token sits exactly
// under its characters. The textarea keeps the caret, selection, IME and
// undo; the editor mirrors its scroll offset onto this layer.
import { forwardRef, memo } from 'react'
import { cn } from '@/lib/utils'
import { tokenize, type Catalog, type TokenKind } from '@/lib/sql'

const CLASS: Record<TokenKind, string> = {
  keyword: 'text-sql-keyword',
  type: 'text-sql-type',
  function: 'text-sql-function',
  string: 'text-sql-string',
  number: 'text-sql-number',
  comment: 'text-sql-comment italic',
  param: 'text-sql-param',
  schema: 'text-sql-table',
  table: 'text-sql-table',
  column: 'text-sql-column',
  operator: 'text-sql-operator',
  plain: ''
}

const Tokens = memo(function Tokens({ sql, catalog }: { sql: string; catalog?: Catalog }) {
  return (
    <>
      {tokenize(sql, catalog).map((t, i) =>
        t.kind === 'plain' ? (
          t.text
        ) : (
          <span key={i} className={CLASS[t.kind]}>
            {t.text}
          </span>
        )
      )}
      {/* A trailing newline needs a character after it to take up a line,
          as it does in the textarea. */}
      {'\n '}
    </>
  )
})

export const SqlHighlight = forwardRef<
  HTMLPreElement,
  { sql: string; catalog?: Catalog; className?: string }
>(function SqlHighlight({ sql, catalog, className }, ref) {
  return (
    <pre
      ref={ref}
      aria-hidden
      className={cn(
        'pointer-events-none absolute inset-0 m-0 overflow-hidden whitespace-pre text-body',
        className
      )}
    >
      <Tokens sql={sql} catalog={catalog} />
    </pre>
  )
})
