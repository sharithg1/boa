// Keys, foreign keys, checks and indexes of an introspected table, shaped
// for the Database Explorer's folders; and DDL built from that metadata for
// the browser mock (the real app asks the server: `table_ddl`).
import type { Constraint, Table } from '@/state/types'
import { qualified, quoteIdent } from './sql'

export type MetaFolderKind = 'keys' | 'foreign keys' | 'indexes' | 'checks'

export type MetaItem = {
  name: string
  /** Short trailing hint: `(org_id) → organizations(id)`. */
  detail: string
  /** The server's definition (constraint or CREATE INDEX). */
  definition: string
  /** The key icon's flavour, for keys / foreign keys / unique indexes. */
  key?: 'primary' | 'unique' | 'foreign'
  constraint?: Constraint
}

export type MetaFolder = { kind: MetaFolderKind; items: MetaItem[] }

const cols = (names: string[]) => `(${names.join(', ')})`

/** Primary key, unique, exclusion, foreign keys, checks (the server's DDL order); stable within a kind. */
const byKind = (cs: Constraint[] = []) =>
  [...cs].sort((a, b) => 'puxfc'.indexOf(a.kind) - 'puxfc'.indexOf(b.kind))

/** DataGrip's folders under a table, in its order; empty ones are left out. */
export function metaFolders(schema: string, t: Table): MetaFolder[] {
  const cons = byKind(t.constraints)
  const of = (...kinds: Constraint['kind'][]) => cons.filter((c) => kinds.includes(c.kind))
  const folders: MetaFolder[] = [
    {
      kind: 'keys',
      items: of('p', 'u', 'x').map((c) => ({
        name: c.name,
        detail: c.kind === 'x' ? c.definition : cols(c.columns),
        definition: c.definition,
        key: c.kind === 'p' ? 'primary' : 'unique',
        constraint: c
      }))
    },
    {
      kind: 'foreign keys',
      items: of('f').map((c) => {
        const r = c.references
        const target = r
          ? r.schema === schema
            ? quoteIdent(r.table)
            : qualified(r.schema, r.table)
          : '?'
        return {
          name: c.name,
          detail: `${cols(c.columns)} → ${target}${r ? cols(r.columns) : ''}`,
          definition: c.definition,
          key: 'foreign',
          constraint: c
        }
      })
    },
    {
      kind: 'indexes',
      items: (t.indexes ?? []).map((i) => ({
        name: i.name,
        detail: `${cols(i.columns)}${i.unique && !i.primary ? ' unique' : ''}`,
        definition: i.definition,
        key: i.primary ? 'primary' : i.unique ? 'unique' : undefined
      }))
    },
    {
      kind: 'checks',
      items: of('c').map((c) => ({
        name: c.name,
        detail: c.definition.replace(/^CHECK\s*/i, ''),
        definition: c.definition,
        constraint: c
      }))
    }
  ]
  return folders.filter((f) => f.items.length > 0)
}

/** Which key a column is part of: the primary key first, then a foreign key. */
export function columnKeys(t: Table): Map<string, 'primary' | 'foreign'> {
  const out = new Map<string, 'primary' | 'foreign'>()
  for (const c of t.constraints ?? [])
    if (c.kind === 'f') for (const n of c.columns) out.set(n, 'foreign')
  for (const c of t.columns) if (c.pk) out.set(c.name, 'primary')
  for (const c of t.constraints ?? [])
    if (c.kind === 'p') for (const n of c.columns) out.set(n, 'primary')
  return out
}

/**
 * DDL shaped like the server's (`table_ddl` in pg/catalog.rs), from the
 * introspected metadata: the browser mock's DDL. Views have no definition
 * in the metadata.
 */
export function ddlFromMetadata(schema: string, t: Table): string {
  const q = qualified(schema, t.name)
  if (t.kind === 'view')
    return `CREATE VIEW ${q} AS\n SELECT ${t.columns.map((c) => quoteIdent(c.name)).join(',\n    ')}\n   FROM …;\n`
  const cons = byKind(t.constraints)
  const lines = [
    ...t.columns.map(
      (c) =>
        `${quoteIdent(c.name)} ${c.type}${c.default ? ` DEFAULT ${c.default}` : ''}${c.nullable ? '' : ' NOT NULL'}`
    ),
    ...cons.map((c) => `CONSTRAINT ${quoteIdent(c.name)} ${c.definition}`)
  ]
  // Metadata without constraints: the columns' primary-key flags.
  const pk = t.columns.filter((c) => c.pk).map((c) => quoteIdent(c.name))
  if (!cons.some((c) => c.kind === 'p') && pk.length) lines.push(`PRIMARY KEY ${cols(pk)}`)
  let out = lines.length
    ? `CREATE TABLE ${q}\n(\n    ${lines.join(',\n    ')}\n);\n`
    : `CREATE TABLE ${q} ();\n`
  // Indexes of primary key / unique / exclusion constraints come with them.
  const owned = new Set(cons.filter((c) => 'pux'.includes(c.kind)).map((c) => c.name))
  const indexes = (t.indexes ?? []).filter((i) => !owned.has(i.name))
  if (indexes.length) out += `\n${indexes.map((i) => `${i.definition};`).join('\n')}\n`
  return out
}
