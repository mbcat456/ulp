# ULP: Unified Leak Pool (.ulp)

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

A self-contained, lossless, searchable credential-store format + toolchain for
`url:username:password` combo lists. Built entirely from raw Rust (std only, no
crates, no zstd/flate), compression and indexing are hand-rolled for this data.

**Why it exists:** a `.txt` combo list is huge and every lookup is a full linear
scan. `.ulp` is smaller on disk *and* answers url / username / password lookups in
microseconds via a custom compressed format + inverted indexes.

---

## Download

Prebuilt Windows and Linux binaries are published on the GitHub Releases page.
Each release includes the binary, README, license, and SHA-256 checksums.

### Install on Windows

```powershell
irm https://raw.githubusercontent.com/mbcat456/ulp/main/install.ps1 | iex
```

### Install on Linux

```bash
curl -fsSL https://raw.githubusercontent.com/mbcat456/ulp/main/install.sh | sh
```

Both installers download the latest release into a user-local directory. Build
from source with the commands below if you prefer a local build.

---

## Setup

### Windows

One command compiles the whole thing (the lib is std-only, no dependencies):

```powershell
cargo build --release
# binary: target/release/ulp.exe
```

### Linux / Unix

The code is cross-platform out of the box via `#[cfg]`; no manual edits needed:

```bash
cargo build --release
# binary: target/release/ulp
```

- `mmap.rs` selects `CreateFileMappingW` on Windows and POSIX `mmap`/`munmap` on
  Unix automatically.
- `peak_rss_gb()` uses `psapi` on Windows and reads `VmHWM` from `/proc/self/status`
  on Linux.
- `guide`'s RAM detection uses `GlobalMemoryStatusEx` (Windows) and `/proc/meminfo`
  (Linux); its free-space check uses `GetDiskFreeSpaceExW` (Windows) and `statvfs`
  (Linux).

The repo is a **library crate plus a thin CLI shim**: the whole toolchain
(build/query/merge/guide/…) is `pub` in the lib, and cred-parser embeds the same
code as its `ulp` subcommand group.

---

## Quick start

> The binary is `ulp` on Linux/macOS and `ulp.exe` on Windows; commands below use
> `./ulp` (substitute `./ulp.exe` on Windows).

```bash
# 1. index a folder of combo lists into one db (auto-picks chunk size from your RAM)
./ulp guide db.ulp lists/

#    ...or manual, explicit files:
./ulp build db.ulp lists/a.txt lists/b.txt

# 2. add more files later without rebuilding (RAM ∝ new data only)
./ulp append db.ulp more/*.txt

# 3. look things up
./ulp query db.ulp url   "netflix.com"       # exact domain
./ulp query db.ulp user  "someone@mail.com"  # exact username
./ulp query db.ulp pass  "hunter2"           # exact password
./ulp match db.ulp contains "roblox"         # substring over domains
```

## Usage

### `query` = exact, `match` = contains / prefix

- `query db.ulp url "netflix.com"` returns only records whose domain is *exactly*
  `netflix.com`. A different subdomain, or a URL that merely *mentions* the word,
  does not match.
- `match db.ulp contains "roblox"` returns every domain containing `roblox` as a
  substring (`roblox.com`, `roblox-hacks.net`, …).
- `match db.ulp prefix "netflix"` matches domains starting with `netflix`.
- `match db.ulp user suffix "@qq.com"` returns every account whose email ends in
  `@qq.com`; `match db.ulp user prefix "admin"` matches emails starting with `admin`.
  (Suffix is the fast path: the username dict is stored reversed, so it's a prefix
  lookup over the reversed index.)

Every query returns full, byte-exact `url:user:pass` lines. Results are **deduplicated**
(identical lines collapse to one) and the summary line reports `found` / `dupes` /
`saved`.

### Save results to a file

The cleanest, shell-independent way is the `-o` flag; the tool writes the file
itself as raw UTF-8, bypassing the shell's redirection entirely:

```bash
./ulp match db.ulp contains "roblox" -o roblox.txt
./ulp query db.ulp url "netflix.com" -o netflix.txt
./ulp dump  db.ulp -o all.txt
```

Timing + `found/dupes/saved` still print to stderr. Plain redirection (`>`) also
works, but the tool always emits UTF-8 to stdout while each shell's `>` re-encodes
differently (PowerShell writes UTF-16), so prefer `-o` for byte-exact output.

### Frames output (`--frames`)

`dump` and `match <contains|prefix>` accept a trailing `--frames` flag (same
positional scan as `-o`): each record is written as three fields, each field
as a u32 little-endian byte length followed by the field bytes:
`[u32 url_len][url][u32 user_len][user][u32 pass_len][pass]`, an empty field
as length 0. Records are concatenated with no header and no count. `dump`
emits all records in store order; `match` emits the matching records in
stream order, deduplicated on the frame bytes (two records differing only in their separator bytes collapse to one frame). Fields round-trip
byte-exactly, so a user containing `:` or `|` survives intact. Misc lines
have no url/user/pass fields, so they have no frame shape and are skipped in
frames mode; when a store holds any, `dump --frames` prints one stderr
warning naming the count (`ulp: frames dump skips N misc rows (no frame
shape); text dump shows them`), a loud warning, never a failure. Text output
remains the default and is unchanged.

### Inspect / rebuild / benchmark

```bash
./ulp info  db.ulp                       # per-segment + total counts
./ulp dump  db.ulp -o all.txt            # lossless reconstruction of every line
./ulp bench db.ulp user "x@y.com" 10000  # in-process lookup timing
```

### Repair a torn store (`repair`)

A kill mid-`append` leaves the old segments intact plus a torn tail (a partial new segment, a partial footer, or no footer at all). `repair` recovers that store in place:

```bash
./ulp repair db.ulp
```

It walks the file structurally from offset 0: header, section offsets, misc entries, every read bounds-checked. It never trusts the existing footer for the cut. Every intact segment is kept, the file is cut at the first invalid one, and a fresh footer is written (a footerless file gets one, so a repaired store never stays in the pointerless legacy shape). A crash mid-append therefore costs the one torn batch, not the store.

- A store whose walk already matches its footer is valid: `repair` exits 0 and touches nothing.
- A file with no valid first segment (foreign bytes, or a fully corrupt store) is refused with exit 1 and left untouched; rebuild it from sources.
- A store with more than 1,000,000 segments is refused (the reader's cap): run `merge` first, then `repair`.
- `repair` validates structure, not payload: bit rot inside a structurally valid segment is not detected.

---

## Guided build (`guide`)

Point it at a folder (or a list of files) and it figures out chunking for you: no
manual batching, no hand-splitting a 30 GB file:

```bash
./ulp guide db.ulp /path/to/combo_lists/      # a folder: every file inside it
./ulp guide db.ulp a.txt b.txt c.txt          # or explicit files
```

It does this, in order:

1. **Detects RAM** with `GlobalMemoryStatusEx` on Windows and `/proc/meminfo` on Linux.
2. **Picks a chunk size** from total + free RAM:
   - **Floor** scales with total RAM (it won't go below this; Windows frees RAM for
     you): 32 GB → 5 GB, 24 → 4 GB, 16 → 3 GB, 8 → 2 GB, else 1 GB.
   - If free RAM is generous it scales **up**: `chunk ≈ (free − 4 GB) / 1.8`, capped at
     16 GB (peak build RAM is ~1.8× the chunk size).
3. **Expands folders** and recurses into any folder argument (non-ASCII filenames handled).
4. **Splits oversized files** when anything bigger than the chunk size is split at **line
   boundaries** (no partial lines) into a `.split.tmp` dir before building.
5. **Checks storage** and estimates the output (~1.5×) and warns if it won't fit at the
   destination.
6. **Builds + appends**, then prints the result.

Example output:

```
[guide] RAM total 31.6 GiB, free 20.0 GiB -> chunk size 8.88 GiB
[guide] 54 files, 117.22 GiB input
[guide] free space 216.5 GiB OK (est output ~78.15 GiB)
[guide] 23 chunks
...
[guide] done -> db.ulp in 12413.2s
[guide] split parts left in db.ulp.split.tmp (safe to delete)
```

For full manual control, `build`/`append` with explicit files still work exactly as
before; `guide` is just a convenience wrapper over them.

---

## Benchmarks (measured)

### Space

| dataset | input | `.ulp` | ratio | contents |
|---|---|---|---|---|
| Secretline.top | 31.96 GB (128 files) | **21.10 GiB** | **1.52×** (34% saved) | 730 M records |
| Alien (TXT_A) | 54.6 GB | 24.93 GB | 2.19× | ~559 M records |

Secretline.top breakdown: **729,917,711 records** → 439.5 M unique usernames,
428.6 M unique passwords, 46.2 M unique domains, 5,761 misc lines, 7 segments.
Peak RSS **8.71 GB** during build (chunked at ~5 GB per segment).

### Lookup

`match contains "roblox"` over the 730 M-record Secretline db:

| method | matches | time |
|---|---|---|
| `.txt` (linear grep, whole-line) | 13,039,203 lines | **623.3 s** (52.5 MB/s) |
| `.ulp` v1.00 (eager dict decode) | 21,545 urls → 12,579,908 lines | **86.8 s** |
| `.ulp` v1.26 (lazy dict decode) | 21,545 urls → 11,584,257 unique | **12.6 s** |

That's **~49× faster than `.txt`** and **~7× faster than v1.00**. The `.ulp` match is
also domain-precise; the grep number is inflated by whole-line hits like
`news.com/roblox-article`. Exact `query` lookups stay in the microseconds (binary
search over the front-coded dictionary).

---

## Commands

```bash
cargo build --release                             # binary: target/release/ulp (ulp.exe on Windows)

./ulp --version                                    # ulp <crate version> (format <n>); scripts pin this
./ulp --help                                       # subcommand + flag summary

./ulp build  db.ulp  in1.txt in2.txt ...            # new database from N files
./ulp append db.ulp  more.txt ...                   # add a segment in-place (RAM ~ new data only)
./ulp repair db.ulp                                 # recover a torn store (cut at the first invalid segment, fresh footer)
./ulp guide  db.ulp  <folder-or-files...>           # guided build: auto chunk size from RAM
./ulp build  db.ulp  --delete-raw lists/*.txt       # delete inputs after a successful index
./ulp query  db.ulp  url|user|pass  <value>  [-o out.txt]  # exact lookup, returns full lines
./ulp match  db.ulp  contains|prefix <keyword> [--frames] [-o out.txt]  # substring/prefix on domain
./ulp match  db.ulp  user <prefix|suffix> <keyword> [-o out.txt]  # email prefix/suffix
./ulp info   db.ulp                                 # per-segment + totals
./ulp info   db.ulp --json                          # same stats, one JSON line (stable field names)
./ulp dump   db.ulp  [--frames] [-o out.txt]        # full lossless reconstruction
./ulp norm   in1.txt in2.txt ...                    # normalized lines (for verification)
./ulp bench  db.ulp  user <value> 10000             # in-process lookup timing
```

### Version and machine-readable info

`./ulp --version` prints `ulp <crate version> (format <format version>)`. The
format version in the parentheses is the `.ulp` on-disk version written into
every segment header. Pin that number for index migration decisions, and bump
it only with a deliberate format change.

`./ulp info db.ulp --json` prints the same stats as one JSON line with stable
snake_case field names, in this order (`--json` may also precede the file):

| field | meaning |
|---|---|
| `file` | path as given |
| `size_bytes` | file size in bytes |
| `format_version` | header format version; mixed-version files are refused |
| `segment_count` | number of segments |
| `records` / `urls` / `users` / `passes` / `misc` | totals |
| `segments` | array of per-segment `{records, urls, users, passes, misc}` |

Field names and order are a pinned API; scripts may rely on them.

### Delete raw inputs after indexing (`--delete-raw`)

`build`, `append`, and `guide` accept a positional `--delete-raw` flag: after a
successful index, each raw input file is deleted. Directories and the `.ulp`
output are never touched, per-file failures print to stderr and the rest
continue, and a summary line reports the counts. The exit code stays 0 because
the index itself succeeded. `guide` also removes its `{out}.split.tmp` dir.

## Format

Fixed 128-byte header per **segment**, then sections:
1. **URL dictionary**: unique **domains only** (scheme/www/path/subdomain stripped),
   stored with **blocked front-coding** (shared prefixes compressed). Multi-part
   ccTLDs preserved (`site.co.uk`, `site.com.mx`).
2. **Username dictionary**: stored **reversed** so shared email domains
   (`@gmail.com`) become shared prefixes and compress.
3. **Password dictionary**: front-coded.
4. **Record table** sorted by URL: `url_offsets` + `user_id`/`pass_id` columns + two
   separator bits (both `:` and `|` stored → byte-exact round-trip).
5. **Inverted indexes** (user→records, pass→records): u32 offsets + delta-varint postings.
6. **Misc section**: non-conforming lines kept verbatim (zero loss): under-delimited lines, `email:pa:ss` pairs (an @-bearing url field without a scheme parks instead of indexing a garbage user), and anything else the splitter cannot classify.

The whole file is a **sequence of self-contained segments + a trailing footer**
(the last 8 bytes point to the footer). `append` builds a new segment and writes it
after the existing ones, updating only the footer; existing bytes are never re-read
or rewritten, so **RAM and I/O are proportional to the new data alone** (a 1 TB store
grown by appends never needs 1 TB of RAM).

## Typed record feed (library)

Embedders that already hold parsed records (cred-parser) can skip the text
parser entirely: `build_typed` / `append_typed` take an in-process iterator of
`(url, user, pass)` byte triples and write the identical record/segment
encoding as text rows with the same dictionaries, same front-coding, and same format
version, so typed and text feeds merge into one store (a `:`-separated text
feed of the same field bytes builds a byte-identical file).

```rust
ulp::build_typed(rows, "db.ulp")?;        // rows: impl IntoIterator<Item = TypedRecord<'_>>
ulp::append_typed("db.ulp", more_rows)?;  // append a segment in place; same errors as `append`
```

- `user` / `pass` bytes are stored exactly as given; `:` / `|` inside them
  never splits the record (the text parser splits at the first two delimiter
  bytes, which silently mis-splits ~3 ppm of a real corpus: 7 rows in 3.5M,
  measured 2026-09-01).
- `url` passes through the same normalization as text rows (registrable domain).
- Misc is structurally impossible: there is no text row to reject, so typed
  segments never hold misc lines.
- The triples are buffered in memory once (the build pipeline passes over its
  source once per dictionary); feeds larger than RAM should use the text path.
- A single field must be under 4 GiB (the record length prefix is u32); an
  oversized field fails the feed with a named io error before anything is
  written.

Dump contract: typed records store `:` for both separators, so `dump` emits
`normalized_url:user:pass`. Records whose user field contains `:` or `|` (or
whose normalized url contains `|`) dump their bytes verbatim, but such a line
no longer re-splits into the original fields under the text parser; queries
stay exact regardless (they resolve dictionary entries, never re-parsed dump
text). The pass field always round-trips: it is everything after the second
separator.

## Lookup path

`query` memory-maps the file (mmap). The OS loads only touched pages; the whole file
is never read. Lookup = binary search over the front-coded dictionary (~20 block
probes) → postings offset jump → record columns → reconstruct. ~3 µs.

## Implementation notes

The current hot path reuses scratch buffers for dictionary search and record
reconstruction, stores dedup slots compactly, batches build writes, sorts merge
runs through contiguous arenas, streams posting blobs through temporary files,
and validates repair metadata with buffered reads. These are internal changes
only: the `.ulp` format, command syntax, and output bytes are unchanged.

## Notes / tradeoffs

- **URL → domain** is a deliberate, user-requested lossy step (only `domain.extension`
  is kept). Username, password, separators, and misc lines are byte-exact. An ad token spliced between a scheme and the path (`https: Engineer: @logsadm //host:...`) strips before the split: the record reconstructs without the junk, the credential itself stays byte-exact.
- Compression is below gzip/zstd (~4×) because `.ulp` trades a little ratio for **O(1)
  indexed lookup**; gzip has no random access. An order-1 Huffman/range-coder over the
  record columns + postings (in 64 KB blocks) would push it toward ~2.3× while keeping
  random access.
- Build RAM ∝ *unique* count (not file size), dominated by the username/password
  dictionaries; use `guide` to chunk inputs to a RAM-appropriate size automatically.

---

## Changelog

Versions are `v1.XX`: bump `+0.01` per small change (bug fix / docs), `+0.10` per
big change (feature / port / major optimization).

| version | what changed |
|---|---|
| **v1.58** | **Feature:** `--version` / `--help` on the bin: `--version` prints bin + crate + `.ulp` format version; `info --json` machine-readable stats with stable snake_case field names. |
| **v1.57** | **Fix:** ad tokens spliced between a scheme and the path strip before the split; @-bearing url fields without a scheme (email pairs) park in misc instead of indexing a garbage user. |
| **v1.56** | **Feature:** `--delete-raw` on build/append/guide deletes raw input files after a successful index (directories and the `.ulp` output never touched; per-file failures print and continue). |
| **v1.55** | **Refactor:** cargo crate: lib + bin split (edition 2024); the whole toolchain is now a reusable library, embedded by cred-parser. |
| **v1.53** | **Feature:** `guide` subcommand: auto-detects RAM (Windows+Linux), picks a chunk size, expands folders, splits oversized files line-aligned, checks free space, then build+append. |
| **v1.43** | **Perf:** dedup `HashSet` uses a passthrough `FoldHasher` (keys are already 128-bit hashes; drops the SipHash re-hash). |
| **v1.42** | **Perf:** `query user`/`query pass` lazy decode (fix for common-password lookups); `info` now prints per-section byte sizes. |
| **v1.41** | **Feature:** `match user prefix\|suffix`: search by email (suffix `@qq.com` is a fast reversed-dict prefix lookup, lazy decode of only referenced entries; ~190× faster than eager). |
| **v1.30** | Docs: `dump` uses `-o` in the inspect section too. |
| **v1.29** | **Feature:** `-o` / `--output` flag on `query`/`match`/`dump`: write results to a file directly (raw UTF-8, shell-independent; no PowerShell UTF-16 surprise). |
| **v1.28** | Docs: platform-neutral commands (`./ulp` + Windows note); added the v1.26 row to the lookup benchmark table. |
| **v1.27** | Docs: added changelog. |
| **v1.26** | **Perf (big):** `match_one` lazy dictionary decode: decode only the user/pass entries referenced by matched records, not the whole multi-hundred-M-entry dictionaries. `match contains` is **~7–10× faster**, byte-identical output (spaggiari 21.8s → 2.2s; roblox 86.8s → 12.6s). |
| **v1.16** | **Bug fixes (4):** `segments_of` bounds-check on the footer offset table (malformed `.ulp` no longer panics); `parse_header` length guard (truncated `.ulp` → clean diagnostic); `merge` sorts the user dict by reversed key (fixes `query user` returning "no match" on merged files); `merge` dedup compares separator bytes (no delimiter-only data loss). |
| **v1.12** | **Docs:** README Linux section reflects native `#[cfg]` cross-platform; documented dedup + `found`/`dupes`/`saved`. |
| **v1.11** | **Big:** cross-platform (Windows + Linux) via `#[cfg]`: ported `mmap.rs` (CreateFileMappingW ↔ POSIX `mmap`) and `peak_rss_gb` (psapi ↔ `/proc/self/status`). |
| **v1.01** | **Feature:** auto-dedup search results (128-bit content hash) + `found`/`dupes`/`saved` stats on every query/match. |
| **v1.00** | Initial release: `.ulp` format, build/append/query/match/dump/info, blocked front-coding + delta-varint compression, mmap lookup. |

## License

MIT. See [LICENSE](LICENSE).
