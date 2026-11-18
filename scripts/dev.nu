#!/usr/bin/env nu
# Start Strata: backend, BFF and Vite, with the development database and seed data.
#
#   nu scripts/dev.nu              development, hot reload on http://127.0.0.1:5173/
#   nu scripts/dev.nu --fresh      start from an empty database
#   nu scripts/dev.nu --no-vite    serve the built frontend from the BFF on :4100
#
# Every flag is passed through to scripts/dev.mjs, which documents them all.
def --wrapped main [...args: string] {
    let root = ($env.FILE_PWD | path dirname)
    cd $root
    if not ("node_modules" | path exists) {
        print "Installing JavaScript dependencies (first run)..."
        ^npm install
    }
    ^node scripts/dev.mjs ...$args
}
