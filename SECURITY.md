# Security

doctopus parses files from untrusted sources, so bugs in its readers can
matter for security. Examples are a crash, unbounded memory or time, or a
file written outside the directory given to `extract`.

Please report such problems privately through GitHub's private
vulnerability reporting: open the repository's **Security** tab and choose
**Report a vulnerability**, or go straight to
<https://github.com/KrisCris/doctopus/security/advisories/new>.
Include the file that triggers the problem if you can.

Files that only make doctopus print an error are not security issues.
Report them as ordinary bugs.
