# AGENTS.md

Use the `ulp` CLI to index text combo lists into `.ulp` files and search them.

Input rows are `url:username:password`. Lines without that shape are kept in a raw misc section and are not searchable.

## Install

Windows:

```powershell
irm https://raw.githubusercontent.com/mbcat456/ulp/main/install.ps1 | iex
```

Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/mbcat456/ulp/main/install.sh | sh
```

## Index

Build one store from explicit files:

```bash
ulp build db.ulp a.txt b.txt
```

Build one store from every file in a folder:

```bash
ulp guide db.ulp lists/
```

Add more files later:

```bash
ulp append db.ulp more.txt
```

Delete raw inputs after successful indexing:

```bash
ulp build db.ulp a.txt --delete-raw
ulp append db.ulp more.txt --delete-raw
ulp guide db.ulp lists/ --delete-raw
```

## Search

Exact lookup:

```bash
ulp query db.ulp url example.com
ulp query db.ulp user someone@example.com
ulp query db.ulp pass hunter2
```

URL substring:

```bash
ulp match db.ulp contains roblox
```

URL prefix:

```bash
ulp match db.ulp prefix netflix
```

Username prefix:

```bash
ulp match db.ulp user prefix admin
```

Username suffix:

```bash
ulp match db.ulp user suffix @qq.com
```

## Output

Write results directly to a file:

```bash
ulp query db.ulp url example.com -o results.txt
ulp match db.ulp contains roblox -o results.txt
```

Prefer `-o`. Shell redirection may change encoding.

## Inspect

Show counts:

```bash
ulp info db.ulp
ulp info db.ulp --json
```

Export all rows:

```bash
ulp dump db.ulp -o all.txt
```

## Notes

- URLs are stored as normalized registrable domains.
- Usernames and passwords are byte-exact.
- `query` is exact.
- `match contains` and `match prefix` operate on normalized URL domains.
- Search results deduplicate identical rows.
- Matching and timing messages go to stderr.
