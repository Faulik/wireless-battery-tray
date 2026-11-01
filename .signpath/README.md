# Setting up SignPath code signing

Releases are signed by SignPath's free program for open-source projects. Until the secret in
step 4 exists, `release.yml` skips signing and ships unsigned exes as before.

1. **Apply.** Fill in the form at https://signpath.org/apply for `Faulik/wireless-battery-tray`
   (MIT, the project site is https://battery.faulcloud.work). SignPath reviews it, which takes
   days to a few weeks. The README's "Code signing policy" section is the policy page they ask for.
2. **Set up the project on signpath.io** once approved:
   - Trusted Build Systems: add the predefined **GitHub.com** one and link it to the project.
   - Project slug: `wireless-battery-tray`.
   - Artifact configuration: paste [artifact-configuration.xml](artifact-configuration.xml) and
     make it the default.
   - Signing policies: SignPath creates `test-signing` and `release-signing`. Add yourself as
     approver on `release-signing`.
   - Optional but recommended: install the SignPath GitHub App on the repository.
3. **Create an API token.** In SignPath, add a CI user (or use your own) with *submitter*
   permission on both signing policies, and copy its API token.
4. **Add it to GitHub.** Repository → Settings → Secrets and variables → Actions:
   - Secret `SIGNPATH_API_TOKEN`: the token from step 3.
   - Variable `SIGNPATH_ORGANIZATION_ID`: the organization ID shown on signpath.io.
   - Optional variables, only if yours differ from the defaults:
     `SIGNPATH_PROJECT_SLUG` (`wireless-battery-tray`),
     `SIGNPATH_SIGNING_POLICY_SLUG` (`release-signing`),
     `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG` (the default configuration).
5. **Test it.** Set `SIGNPATH_SIGNING_POLICY_SLUG` to `test-signing` and rerun the Release workflow
   for an existing tag (Actions → Release → Run workflow). Test-signed exes use an untrusted
   certificate, so the workflow only warns about them. Delete the variable afterwards.
6. **Each release:** push the tag as usual, then approve the signing request on signpath.io (you
   get an email). The workflow waits up to an hour for it.

Even signed, a brand-new release can still trigger SmartScreen until enough people download it.
The signature builds that reputation, and SignPath's certificate is shared by many projects, so
it usually fades quickly.
