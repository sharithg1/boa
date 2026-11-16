// In-browser sample data, served by the mock backend (lib/backend.ts) when
// the UI runs in a plain browser: a few connections with databases, schemas
// and tables.
import type { Column, Connection, Constraint, Database, Index, Table } from './types'

/** A sample server: a connection without the saved-settings fields. */
export type SampleConnection = Omit<Connection, 'database' | 'sslMode' | 'hasPassword' | 'readOnly'>

const col = (
  name: string,
  type: string,
  opts: Partial<Omit<Column, 'name' | 'type'>> = {}
): Column => ({ name, type, nullable: false, pk: false, default: null, ...opts })

const id = col('id', 'bigint', { pk: true, default: "nextval('…_id_seq')" })
const ts = (name: string) => col(name, 'timestamptz', { default: 'now()' })

const table = (name: string, rows: number, columns: Column[], kind: Table['kind'] = 'table') => ({
  name,
  kind,
  rows,
  sizeBytes: rows * (columns.length * 18 + 24),
  columns
})

/** Keys and indexes of one sample table, besides the primary key (from the `pk` columns). */
type Keys = {
  unique?: string[][]
  /** `ref`: `table` in the same schema, or `schema.table`; referenced columns default to `id`. */
  fks?: { cols: string[]; ref: string; refCols?: string[]; onDelete?: string }[]
  checks?: { name: string; expr: string; cols: string[] }[]
  /** `keys`: the parenthesised key list when it isn't just `cols` (`created_at DESC`). */
  indexes?: { name: string; cols: string[]; keys?: string; using?: string }[]
}

/** Adds Postgres-style constraints and indexes (as introspection returns them) to `db`'s tables. */
function withKeys(db: Database, keys: Record<string, Keys>) {
  const list = (cs: string[]) => cs.join(', ')
  for (const s of db.schemas) {
    for (const t of s.tables) {
      if (t.kind !== 'table') continue
      const k = keys[`${s.name}.${t.name}`] ?? {}
      const constraints: Constraint[] = []
      const indexes: Index[] = []
      const index = (
        name: string,
        cols: string[],
        unique: boolean,
        primary = false,
        keyList = list(cols),
        using = 'btree'
      ) =>
        indexes.push({
          name,
          columns: cols,
          unique,
          primary,
          definition: `CREATE ${unique ? 'UNIQUE ' : ''}INDEX ${name} ON ${s.name}.${t.name} USING ${using} (${keyList})`
        })
      const pk = t.columns.filter((c) => c.pk).map((c) => c.name)
      if (pk.length) {
        const name = `${t.name}_pkey`
        constraints.push({ name, kind: 'p', columns: pk, definition: `PRIMARY KEY (${list(pk)})` })
        index(name, pk, true, true)
      }
      for (const cols of k.unique ?? []) {
        const name = `${t.name}_${cols.join('_')}_key`
        constraints.push({ name, kind: 'u', columns: cols, definition: `UNIQUE (${list(cols)})` })
        index(name, cols, true)
      }
      for (const fk of k.fks ?? []) {
        const [schema, table] = fk.ref.includes('.') ? fk.ref.split('.') : [s.name, fk.ref]
        const refCols = fk.refCols ?? ['id']
        const target = schema === s.name ? table : `${schema}.${table}`
        constraints.push({
          name: `${t.name}_${fk.cols.join('_')}_fkey`,
          kind: 'f',
          columns: fk.cols,
          definition: `FOREIGN KEY (${list(fk.cols)}) REFERENCES ${target}(${list(refCols)})${fk.onDelete ? ` ON DELETE ${fk.onDelete}` : ''}`,
          references: { schema: schema!, table: table!, columns: refCols }
        })
      }
      for (const c of k.checks ?? [])
        constraints.push({
          name: c.name,
          kind: 'c',
          columns: c.cols,
          definition: `CHECK (${c.expr})`
        })
      for (const i of k.indexes ?? []) index(i.name, i.cols, false, false, i.keys, i.using)
      t.constraints = constraints.sort((a, b) => a.name.localeCompare(b.name))
      t.indexes = indexes
    }
  }
}

const APP_DB: Database = {
  name: 'app_production',
  schemas: [
    {
      name: 'public',
      tables: [
        table('users', 182_340, [
          id,
          col('email', 'text'),
          col('name', 'text', { nullable: true }),
          col('org_id', 'bigint', { nullable: true }),
          col('role', 'user_role', { default: "'member'" }),
          ts('created_at'),
          ts('updated_at')
        ]),
        table('organizations', 9_412, [
          id,
          col('name', 'text'),
          col('plan', 'text', { default: "'free'" }),
          ts('created_at')
        ]),
        table('invoices', 1_204_551, [
          id,
          col('org_id', 'bigint'),
          col('amount_cents', 'integer'),
          col('currency', 'char(3)', { default: "'USD'" }),
          col('status', 'invoice_status'),
          col('paid_at', 'timestamptz', { nullable: true }),
          ts('created_at')
        ]),
        table('sessions', 3_880_102, [
          col('token', 'uuid', { pk: true }),
          col('user_id', 'bigint'),
          col('expires_at', 'timestamptz'),
          ts('created_at')
        ]),
        table(
          'active_users',
          41_220,
          [id, col('email', 'text'), col('last_seen_at', 'timestamptz')],
          'view'
        )
      ]
    },
    {
      name: 'audit',
      tables: [
        table('events', 22_901_337, [
          id,
          col('actor_id', 'bigint', { nullable: true }),
          col('action', 'text'),
          col('payload', 'jsonb'),
          ts('at')
        ])
      ]
    }
  ]
}

withKeys(APP_DB, {
  'public.users': {
    unique: [['email']],
    fks: [{ cols: ['org_id'], ref: 'organizations', onDelete: 'SET NULL' }],
    indexes: [{ name: 'users_org_id_idx', cols: ['org_id'] }]
  },
  'public.organizations': {
    checks: [
      {
        name: 'organizations_plan_check',
        expr: "plan = ANY (ARRAY['free'::text, 'team'::text, 'enterprise'::text])",
        cols: ['plan']
      }
    ]
  },
  'public.invoices': {
    fks: [{ cols: ['org_id'], ref: 'organizations', onDelete: 'CASCADE' }],
    checks: [
      { name: 'invoices_amount_cents_check', expr: 'amount_cents >= 0', cols: ['amount_cents'] }
    ],
    indexes: [
      {
        name: 'invoices_org_id_created_at_idx',
        cols: ['org_id', 'created_at'],
        keys: 'org_id, created_at DESC'
      }
    ]
  },
  'public.sessions': {
    fks: [{ cols: ['user_id'], ref: 'users', onDelete: 'CASCADE' }],
    indexes: [
      { name: 'sessions_user_id_idx', cols: ['user_id'] },
      { name: 'sessions_expires_at_idx', cols: ['expires_at'] }
    ]
  },
  'audit.events': {
    fks: [{ cols: ['actor_id'], ref: 'public.users' }],
    indexes: [
      { name: 'events_payload_idx', cols: ['payload'], using: 'gin' },
      { name: 'events_at_idx', cols: ['at'], using: 'brin' }
    ]
  }
})

const ANALYTICS_DB: Database = {
  name: 'analytics',
  schemas: [
    {
      name: 'public',
      tables: [
        table('page_views', 98_120_004, [
          id,
          col('path', 'text'),
          col('user_id', 'bigint', { nullable: true }),
          col('referrer', 'text', { nullable: true }),
          ts('at')
        ]),
        table('daily_rollups', 2_190, [
          col('day', 'date', { pk: true }),
          col('views', 'bigint'),
          col('uniques', 'bigint')
        ])
      ]
    }
  ]
}

withKeys(ANALYTICS_DB, {
  'public.page_views': {
    indexes: [{ name: 'page_views_user_id_at_idx', cols: ['user_id', 'at'] }]
  },
  'public.daily_rollups': {
    checks: [{ name: 'daily_rollups_check', expr: 'uniques <= views', cols: ['views', 'uniques'] }]
  }
})

const LOCAL_DB: Database = {
  name: 'main',
  schemas: [
    {
      name: 'main',
      tables: [
        table('todos', 42, [
          col('id', 'INTEGER', { pk: true }),
          col('title', 'TEXT'),
          col('done', 'INTEGER', { default: '0' })
        ]),
        table('tags', 7, [col('id', 'INTEGER', { pk: true }), col('label', 'TEXT')])
      ]
    }
  ]
}

const SHOP_DB: Database = {
  name: 'shop',
  schemas: [
    {
      name: 'shop',
      tables: [
        table('products', 12_004, [
          col('id', 'int unsigned', { pk: true }),
          col('sku', 'varchar(64)'),
          col('title', 'varchar(255)'),
          col('price', 'decimal(10,2)')
        ]),
        table('orders', 310_877, [
          col('id', 'int unsigned', { pk: true }),
          col('customer_id', 'int unsigned'),
          col('total', 'decimal(10,2)'),
          col('placed_at', 'datetime')
        ])
      ]
    }
  ]
}

export function sampleConnections(): SampleConnection[] {
  return [
    {
      id: 'prod-pg',
      name: 'Production',
      engine: 'postgres',
      host: 'db.internal',
      port: 5432,
      user: 'readonly',
      state: 'connected',
      latencyMs: 18,
      serverVersion: 'PostgreSQL 17.2',
      databases: [APP_DB, ANALYTICS_DB]
    },
    {
      id: 'staging-mysql',
      name: 'Staging',
      engine: 'mysql',
      host: 'staging-mysql.internal',
      port: 3306,
      user: 'app',
      state: 'disconnected',
      latencyMs: null,
      serverVersion: 'MySQL 8.4.3',
      databases: [SHOP_DB]
    },
    {
      id: 'local-sqlite',
      name: 'todos.db',
      engine: 'sqlite',
      host: '~/Projects/todos/todos.db',
      port: null,
      user: null,
      state: 'connected',
      latencyMs: 0,
      serverVersion: 'SQLite 3.47',
      databases: [LOCAL_DB]
    }
  ]
}
