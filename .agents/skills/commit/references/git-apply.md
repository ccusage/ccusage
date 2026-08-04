# Git Apply Reference

Stage a hand-built patch without touching the worktree, verifying first:

```bash
git apply --check patch_file.patch
git apply --cached -v patch_file.patch
```

Keep `-v` so a failure reports which hunk was rejected. `--stat` lists the
affected files before applying.

When a patch does not apply cleanly:

- Trailing whitespace: `--whitespace=fix`
- Only some hunks conflict: `--reject` writes `.rej` files instead of aborting
- Context mismatch: `--ignore-whitespace`
- Line-ending differences: `--ignore-space-change`
- Undo an applied patch: `--reverse`
