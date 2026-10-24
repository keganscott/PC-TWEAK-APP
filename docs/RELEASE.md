# Releasing PeakTweaks: installer and code signing

## The installers

Every Windows CI run builds two installers and uploads them as the artifact **peaktweaks-installers**:

| File | What it is |
|---|---|
| `PeakTweaks-Setup-<version>.exe` | The release build. While licensing (Phase 7) does not exist, it unlocks only the Free tools. |
| `PeakTweaks-Tester-Setup-<version>.exe` | The tester build: every plan unlocked, "Tester build" on every screen (DECISIONS 15.21). For Kegan and testers until licensing lands. |

What the installer does (`scripts/build-installers.ps1`, Tauri's NSIS bundler, `src-tauri/tauri.conf.json > bundle`):
- Installs for all users into `C:\Program Files\PeakTweaks`.
- Creates Start menu and desktop shortcuts with the PeakTweaks icon.
- Adds PeakTweaks to Settings > Apps, with an uninstaller.
- Installs WebView2 if the PC lacks it. This downloads Microsoft's installer, the only network use, and only during setup.

CI installs each one silently, checks the files, the admin manifest, the shortcuts and the signature, then uninstalls it again. Uninstalling removes the program; it does not undo the user's changes. Undo all in Backups does that, and the change record in `C:\ProgramData\PeakTweaks` stays so it still can.

## Code signing

Without a signature, Windows shows "Windows protected your PC" on download and first launch, which loses most first-time users. With one, the publisher shows as PeakTweaks. Like every new certificate, SmartScreen still warns until the files build up download reputation; that takes weeks of real downloads.

### Recommended: Azure Artifact Signing (formerly Trusted Signing)

About **$9.99 a month**, and no hardware token. Individuals: USA and Canada only; organizations: USA, Canada, EU and UK. Microsoft verifies the identity, which takes a few business days.

What Kegan does once:
1. Create an Azure account (portal.azure.com) with a subscription.
2. Create an **Artifact Signing account** (Basic tier) in a region, and note its **endpoint** (for example `https://eus.codesigning.azure.net`) and **account name**.
3. Complete **identity validation** (individual or organization) in that account.
4. Create a **certificate profile** of type *Public Trust*, and note its name.
5. In Microsoft Entra ID, create an **app registration** with a client secret. In the Artifact Signing account, give it the role **Artifact Signing Certificate Profile Signer**.
6. In GitHub, go to the repository, then Settings, then Secrets and variables, then Actions, and add:
   - Secrets: `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`.
   - Variables: `SIGNING_ENDPOINT`, `SIGNING_ACCOUNT`, `SIGNING_PROFILE`.

From the next CI run on, both installers and the app inside them are signed, and CI fails if the signature is not valid.

VERIFY: the `trusted-signing-cli` flags (`-e`, `-a`, `-c`, `-d`) and the role name are from the tool's README and Microsoft's documentation as found on 2026-10-22, and have not been run yet. The first signed CI run settles them.

### Alternative: an OV certificate from a certificate authority

About $200-400 a year (for example Sectigo, DigiCert, SSL.com). Since 2023 the key must live on a hardware token or a cloud HSM. The cloud kind (for example SSL.com eSigner) works in CI through `bundle > windows > signCommand`, the same hook the script uses.

## Sources

- [Microsoft: code signing options](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options)
- [Azure Artifact Signing pricing](https://azure.microsoft.com/pricing/details/artifact-signing/)
- [Tauri: Windows code signing](https://v2.tauri.app/distribute/sign/windows/)
- [trusted-signing-cli](https://github.com/Levminer/trusted-signing-cli)
