// "DDL" on a table: asks the backend for the relation's DDL (the server's
// view of it; the browser mock builds it from the sample metadata) and opens
// it in a new console on that database.
import { backend, toBackendError } from '@/lib/backend'
import { newQueryTab, pushToast } from './store'

export async function openDdl(
  connectionId: string,
  database: string,
  schema: string,
  table: string
) {
  try {
    const sql = await backend.tableDdl(connectionId, database, schema, table)
    newQueryTab(connectionId, database, sql)
  } catch (e) {
    pushToast({
      kind: 'error',
      title: `No DDL for ${schema}.${table}`,
      body: toBackendError(e).message
    })
  }
}
