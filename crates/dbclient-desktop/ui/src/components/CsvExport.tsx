// CSV export UI: the Export menu of the result grid and table toolbars
// (Export to CSV…, Copy page as CSV, delimiter and header options), and the
// status bar indicator of running exports with their progress and Cancel.
import { Copy, Download, FileDown, X } from 'lucide-react'
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger
} from '@/components/ui/dropdown-menu'
import { Spinner } from '@/components/ui/spinner'
import { CSV_DELIMITERS, type CsvDelimiter } from '@/lib/csv'
import { bytes } from '@/lib/format'
import {
  cancelExport,
  copyCsv,
  type ExportSource,
  exportCsv,
  setCsvOptions,
  useCsvExport
} from '@/state/csvExport'
import { IconButton } from './IconButton'

/** Export menu for a result (or table page); `owner` ties a running export to it. */
export function ExportMenu({
  source,
  owner,
  disabled
}: {
  source: ExportSource | undefined
  owner: string
  disabled?: boolean
}) {
  const options = useCsvExport((s) => s.options)
  const job = useCsvExport((s) => s.jobs.find((j) => j.owner === owner))
  if (job) {
    return (
      <span className="flex shrink-0 items-center gap-1 pl-1.5 text-xs text-dim">
        <Spinner className="size-3" />
        Exporting…
        <IconButton label="Cancel export" side="top" onClick={() => cancelExport(job.id)}>
          <X className="size-3.5" />
        </IconButton>
      </span>
    )
  }
  const loaded = source?.rows?.length ?? 0
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        aria-label="Export"
        title="Export"
        disabled={disabled || !source}
        className="grid size-6 shrink-0 cursor-pointer place-items-center rounded-md text-muted-foreground outline-none hover:bg-hover-strong hover:text-foreground disabled:cursor-default disabled:opacity-40 data-[state=open]:bg-hover-strong"
      >
        <Download className="size-3.5" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-60">
        <DropdownMenuItem onSelect={() => source && void exportCsv(source, owner)}>
          <FileDown />
          Export to CSV…
        </DropdownMenuItem>
        <DropdownMenuItem
          disabled={!source?.rows || !source.columns}
          onSelect={() => source?.columns && void copyCsv(source.columns, source.rows ?? [])}
        >
          <Copy />
          Copy {source?.pageable ? 'page' : 'rows'} as CSV
          <span className="ml-auto text-xs text-dim">{loaded.toLocaleString()}</span>
        </DropdownMenuItem>
        {source && !source.pageable && (
          <div className="px-2 py-1 text-xs leading-snug text-dim">
            The statement changes data: only the loaded rows are exported.
          </div>
        )}
        <DropdownMenuSeparator />
        <DropdownMenuLabel className="text-xs text-dim">Delimiter</DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={options.delimiter}
          onValueChange={(v) => setCsvOptions({ delimiter: v as CsvDelimiter })}
        >
          {CSV_DELIMITERS.map((d) => (
            <DropdownMenuRadioItem
              key={d.label}
              value={d.value}
              onSelect={(e) => e.preventDefault()}
            >
              {d.label}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <DropdownMenuCheckboxItem
          checked={options.header}
          onCheckedChange={(header) => setCsvOptions({ header })}
          onSelect={(e) => e.preventDefault()}
        >
          Header row
        </DropdownMenuCheckboxItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/** Running exports, for the status bar. */
export function ExportStatus() {
  const jobs = useCsvExport((s) => s.jobs)
  const job = jobs[0]
  if (!job) return null
  return (
    <span className="flex min-w-0 items-center gap-1.5 px-1.5 text-xs text-soft">
      <Spinner className="size-3 shrink-0" />
      <span className="truncate">
        Exporting <span className="font-mono">{job.file}</span>
        {job.bytes > 0 && (
          <span className="text-dim">
            {' '}
            · {job.rows.toLocaleString()} rows · {bytes(job.bytes)}
          </span>
        )}
        {jobs.length > 1 && <span className="text-dim"> (+{jobs.length - 1})</span>}
      </span>
      <IconButton label="Cancel export" side="top" onClick={() => cancelExport(job.id)}>
        <X className="size-3.5" />
      </IconButton>
    </span>
  )
}
