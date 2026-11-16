import { describe, expect, it } from 'vitest'
import type { Column, Table } from '@/state/types'
import { columnKeys, ddlFromMetadata, metaFolders } from './tableMeta'

const col = (name: string, type: string, opts: Partial<Column> = {}): Column => ({
  name,
  type,
  nullable: false,
  pk: false,
  default: null,
  ...opts
})

const invoices: Table = {
  name: 'invoices',
  kind: 'table',
  rows: 0,
  sizeBytes: 0,
  columns: [
    col('id', 'bigint', { pk: true }),
    col('org_id', 'bigint'),
    col('actor_id', 'bigint', { nullable: true }),
    col('amount', 'integer', { default: '0' })
  ],
  constraints: [
    { name: 'invoices_pkey', kind: 'p', columns: ['id'], definition: 'PRIMARY KEY (id)' },
    {
      name: 'invoices_org_id_fkey',
      kind: 'f',
      columns: ['org_id'],
      definition: 'FOREIGN KEY (org_id) REFERENCES orgs(id)',
      references: { schema: 'public', table: 'orgs', columns: ['id'] }
    },
    {
      name: 'invoices_actor_id_fkey',
      kind: 'f',
      columns: ['actor_id'],
      definition: 'FOREIGN KEY (actor_id) REFERENCES auth."Users"(id)',
      references: { schema: 'auth', table: 'Users', columns: ['id'] }
    },
    {
      name: 'invoices_amount_check',
      kind: 'c',
      columns: ['amount'],
      definition: 'CHECK (amount >= 0)'
    }
  ],
  indexes: [
    {
      name: 'invoices_pkey',
      columns: ['id'],
      unique: true,
      primary: true,
      definition: 'CREATE UNIQUE INDEX invoices_pkey ON public.invoices USING btree (id)'
    },
    {
      name: 'invoices_lower_idx',
      columns: ['org_id', 'lower(note)'],
      unique: true,
      primary: false,
      definition:
        'CREATE UNIQUE INDEX invoices_lower_idx ON public.invoices USING btree (org_id, lower(note))'
    }
  ]
}

describe('metaFolders', () => {
  it('groups keys, foreign keys, indexes and checks, in that order', () => {
    const f = metaFolders('public', invoices)
    expect(f.map((x) => x.kind)).toEqual(['keys', 'foreign keys', 'indexes', 'checks'])
    expect(f[0]!.items).toMatchObject([{ name: 'invoices_pkey', detail: '(id)', key: 'primary' }])
    expect(f[1]!.items.map((i) => i.detail)).toEqual([
      '(org_id) → orgs(id)',
      '(actor_id) → auth."Users"(id)'
    ])
    expect(f[2]!.items.map((i) => [i.detail, i.key])).toEqual([
      ['(id)', 'primary'],
      ['(org_id, lower(note)) unique', 'unique']
    ])
    expect(f[3]!.items[0]!.detail).toBe('(amount >= 0)')
  })

  it('leaves out empty folders', () => {
    const bare: Table = { ...invoices, constraints: undefined, indexes: [] }
    expect(metaFolders('public', bare)).toEqual([])
  })
})

describe('columnKeys', () => {
  it('marks primary-key and foreign-key columns', () => {
    const k = columnKeys(invoices)
    expect(Object.fromEntries(k)).toEqual({
      id: 'primary',
      org_id: 'foreign',
      actor_id: 'foreign'
    })
  })
})

describe('ddlFromMetadata', () => {
  it('builds CREATE TABLE with constraints, then the other indexes', () => {
    expect(ddlFromMetadata('public', invoices)).toBe(
      `CREATE TABLE public.invoices
(
    id bigint NOT NULL,
    org_id bigint NOT NULL,
    actor_id bigint,
    amount integer DEFAULT 0 NOT NULL,
    CONSTRAINT invoices_pkey PRIMARY KEY (id),
    CONSTRAINT invoices_org_id_fkey FOREIGN KEY (org_id) REFERENCES orgs(id),
    CONSTRAINT invoices_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES auth."Users"(id),
    CONSTRAINT invoices_amount_check CHECK (amount >= 0)
);

CREATE UNIQUE INDEX invoices_lower_idx ON public.invoices USING btree (org_id, lower(note));
`
    )
  })

  it('falls back to the columns’ primary-key flags, and quotes names', () => {
    const t: Table = {
      name: 'Odd',
      kind: 'table',
      rows: 0,
      sizeBytes: 0,
      columns: [col('id', 'int', { pk: true }), col('select', 'text', { nullable: true })]
    }
    expect(ddlFromMetadata('my schema', t)).toBe(
      'CREATE TABLE "my schema"."Odd"\n(\n    id int NOT NULL,\n    "select" text,\n    PRIMARY KEY (id)\n);\n'
    )
  })

  it('outlines a view', () => {
    const v: Table = { ...invoices, kind: 'view', columns: [col('id', 'bigint')] }
    expect(ddlFromMetadata('public', v)).toBe(
      'CREATE VIEW public.invoices AS\n SELECT id\n   FROM …;\n'
    )
  })
})
