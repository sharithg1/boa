// In-browser sample data, served by the mock backend (lib/backend.ts) when
// the UI runs in a plain browser: a few connections with databases, schemas
// and tables.
import type { Column, Connection, Database, Table } from './types'

/** A sample server: a connection without the saved-settings fields. */
export type SampleConnection = Omit<Connection, 'database' | 'sslMode' | 'hasPassword'>

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
