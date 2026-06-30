# Security policy

## Reporting a vulnerability

If you find a security issue in Formant, please report it privately rather than
opening a public issue.

Use GitHub's private vulnerability reporting on this repository (the Security tab,
"Report a vulnerability"), or email the maintainer. Include enough detail to
reproduce the problem.

Please give a reasonable amount of time for a fix before any public disclosure.

## Scope

Formant runs locally and does not make network connections on its own. The most
relevant areas are the WASAPI audio handling, the VST3 plugin host (which loads
third-party native code that you choose to install), and the file parsing for
presets, sessions, and config.

## Supported versions

Formant is pre-1.0. Fixes land on the latest release.
