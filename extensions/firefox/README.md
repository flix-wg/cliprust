# ClipRust Source for Firefox

An optional add-on for Firefox and Firefox-based browsers such as Zen. It adds the page address to text you copy, so ClipRust shows where a clip came from and can use the site to recognise keys. Chromium browsers (Chrome, Arc, Edge, Brave) already do this on their own, so they don't need it.

ClipRust works the same without it; Firefox clips just have no source.

## How it works

A content script listens for `copy` on every http(s) page and adds the page URL as `text/x-cliprust-source`. Firefox stores custom clipboard types in `org.mozilla.custom-clipdata`, and ClipRust's capture reads the value from there.

- If the page handles the copy itself, the add-on only adds its type.
- Otherwise Firefox would ignore any added data, so the add-on writes the selection as plain text and HTML itself and cancels the default copy.
- Password fields are left alone, and copies the page stops from propagating get no source.

It has no background script, no network access and no storage, and asks for no permissions beyond running on web pages. A page can set the same type itself, so treat the source as a hint, the same as the Chromium one.

## Try it

1. Open `about:debugging#/runtime/this-firefox` in Firefox or Zen.
2. Click **Load Temporary Add-on…** and pick `manifest.json` in this folder.
3. Copy some text on a web page; ClipRust shows the site in the preview's Source row.

Temporary add-ons are removed when the browser quits. To keep it installed, use a signed build (below).

## Signing and listing on addons.mozilla.org

Release Firefox only installs add-ons signed by Mozilla. Signing is free and needs an AMO account and API credentials (Developer Hub → Tools → Manage API Keys):

```sh
npx web-ext lint --source-dir extensions/firefox
npx web-ext sign --source-dir extensions/firefox --channel unlisted \
  --api-key "$AMO_JWT_ISSUER" --api-secret "$AMO_JWT_SECRET"
```

- **Unlisted** returns a signed `.xpi` after an automated check, usually within minutes. Attach it to the GitHub release; users install it by opening the file in Firefox. Updates need an `update_url` in the manifest or a manual reinstall.
- **Listed** (`--channel listed`) publishes it on addons.mozilla.org with automatic updates. Because the content script runs on all sites it may get a manual review. The listing needs a summary, a category (Other), a 128 px icon and a privacy note; the manifest already declares that no data is collected (`data_collection_permissions: none`).
- The source is plain, unbundled JavaScript, so no separate source upload is needed.
- Bump `version` in `manifest.json` for every upload; AMO rejects a version it has seen.
- The add-on ID `source@cliprust.app` is fixed once signed; changing it makes a new add-on.
