# breached-passwords.txt

New passwords found here are refused (NIST SP 800-63B §3.1.1.2, OWASP ASVS
V2.1.7). It is every password of 12 to 256 characters in SecLists'
`Passwords/Common-Credentials/Pwdb_top-1000000.txt` (the million most common
passwords in breach compilations), lowercased, deduplicated and sorted.
Shorter ones are already refused by the length rule.

Source: https://github.com/danielmiessler/SecLists (MIT License, Copyright
(c) 2018 Daniel Miessler).

To regenerate, filter that file the same way:

```sh
python3 -c '
import sys
words = {l.rstrip("\r\n").lower() for l in open(sys.argv[1], encoding="utf-8", errors="ignore")}
print("\n".join(sorted(w for w in words if 12 <= len(w) <= 256)))
' Pwdb_top-1000000.txt > breached-passwords.txt
```
