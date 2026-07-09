chore: release {{.NextVersion}}

This pull request is for the next release as {{.NextVersion}} created by [tagpr](https://github.com/Songmu/tagpr). Merging it will tag {{.NextVersion}} to the merge commit and create a GitHub release.

You can modify this branch "{{.Branch}}" directly before merging if you want to change the next version number or other files for the release.

<details>
<summary>How to change the next version as you like</summary>

There are two ways to do it.

- Version file
  - Edit and commit the version file specified in the .tagpr configuration file to describe the next version
  - If you want to use another version file, edit the configuration file.
- Labels convention
  - Add labels to this pull request like "tagpr:minor" or "tagpr:major"
  - If no conventional labels are added, the patch version is incremented as is.

</details>

---

{{.Changelog}}
