// Highlight layer of the SQL editor: a <pre> under a transparent textarea,
// same font, padding and no wrapping, so each coloured token sits exactly
// under its characters. The textarea keeps the caret, selection, IME and
// undo; the editor mirrors its scroll offset onto this layer. `marks` are
// SVG outlines / fills over statement ranges (lib/statements.ts rangePath),
// drawn in content coordinates so they scroll with the text.
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

/** Highlighted SQL inline (no editor layer), e.g. in the query history list. */
export const SqlText = memo(function SqlText({ sql, catalog }: { sql: string; catalog?: Catalog }) {
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
    </>
  )
})

export type EditorMark = { d: string; kind: 'outline' | 'fill' }

export const SqlHighlight = forwardRef<
  HTMLPreElement,
  { sql: string; catalog?: Catalog; className?: string; marks?: EditorMark[] }
>(function SqlHighlight({ sql, catalog, className, marks }, ref) {
  return (
    <pre
      ref={ref}
      aria-hidden
      className={cn(
        'pointer-events-none absolute inset-0 m-0 overflow-hidden whitespace-pre text-body',
        className
      )}
    >
      {!!marks?.length && (
        <svg
          aria-hidden
          width={1}
          height={1}
          className="pointer-events-none absolute top-0 left-0 overflow-visible"
        >
          {marks.map((m, i) => (
            <path
              key={i}
              d={m.d}
              className={
                m.kind === 'fill'
                  ? 'fill-status-ok/15 stroke-status-ok/70'
                  : 'fill-none stroke-status-ok/45'
              }
              strokeWidth={1}
              shapeRendering="crispEdges"
            />
          ))}
        </svg>
      )}
      <Tokens sql={sql} catalog={catalog} />
    </pre>
  )
})
