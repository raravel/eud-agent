// Resolve the latest installers from GitHub Releases and wire up the download
// buttons + version labels. Falls back to the releases/latest page if the API
// is unreachable (offline, rate-limited), so every link always works.
(function () {
  "use strict";

  var REPO = "raravel/eud-agent";
  var RELEASES_PAGE = "https://github.com/" + REPO + "/releases/latest";
  // List endpoint (not /releases/latest): the RAG index ships as its own
  // `rag-index-v*` release with no installer, and it can become GitHub's
  // "latest". Scan the list and pick the newest release that actually carries
  // the program installer, ignoring RAG-index-only releases.
  var API = "https://api.github.com/repos/" + REPO + "/releases?per_page=30";

  // The visitor's platform picks the primary button and the default install
  // tab. demo.js reads the same attribute.
  var ua = navigator.userAgent || "";
  var os = /Macintosh|Mac OS X/i.test(ua) && !/iPhone|iPad/i.test(ua) ? "mac" : "win";
  document.documentElement.setAttribute("data-os", os);

  var btn = document.getElementById("download-btn");
  var label = document.getElementById("download-label");
  var versionEls = document.querySelectorAll("[data-version]");
  var dateEl = document.getElementById("pubdate");
  var links = document.querySelectorAll("[data-dl]");

  if (label && os === "mac") label.textContent = "macOS용 다운로드";

  function setVersion(text) {
    for (var i = 0; i < versionEls.length; i++) {
      versionEls[i].textContent = text;
    }
  }

  // Pre-set the fallback so the button is functional before/without the API.
  if (btn) btn.href = RELEASES_PAGE;

  function pick(assets, pattern) {
    for (var i = 0; i < assets.length; i++) {
      var name = assets[i].name || "";
      if (pattern.test(name) && !/\.sig$/i.test(name)) return assets[i];
    }
    return null;
  }

  fetch(API, { headers: { Accept: "application/vnd.github+json" } })
    .then(function (res) {
      if (!res.ok) throw new Error("GitHub API " + res.status);
      return res.json();
    })
    .then(function (releases) {
      if (!Array.isArray(releases)) throw new Error("unexpected response");

      // Releases come back newest-first; take the first one (skipping drafts
      // and prereleases, matching /releases/latest semantics) that bundles the
      // Windows installer. RAG-index releases carry no `-setup.exe`, so they
      // are skipped here.
      var release = null;
      for (var r = 0; r < releases.length; r++) {
        if (releases[r].draft || releases[r].prerelease) continue;
        if (pick(releases[r].assets || [], /-setup\.exe$/i)) {
          release = releases[r];
          break;
        }
      }
      if (!release) throw new Error("no installer release found");

      var assets = release.assets || [];
      var urls = {
        win: pick(assets, /-setup\.exe$/i),
        "mac-arm": pick(assets, /(aarch64|arm64)\.dmg$/i),
        "mac-x64": pick(assets, /(x64|x86_64)\.dmg$/i),
      };

      for (var i = 0; i < links.length; i++) {
        var asset = urls[links[i].getAttribute("data-dl")];
        if (asset) links[i].href = asset.browser_download_url;
      }
      // A Mac visitor's primary button gets the Apple Silicon build; the
      // install section offers the Intel one next to it.
      var primary = os === "mac" ? urls["mac-arm"] : urls.win;
      if (primary && btn) btn.href = primary.browser_download_url;

      var tag = release.tag_name || "";
      if (tag) setVersion(tag);

      if (release.published_at && dateEl) {
        var d = new Date(release.published_at);
        if (!isNaN(d.getTime())) {
          dateEl.textContent = d.toLocaleDateString("ko-KR", {
            year: "numeric",
            month: "long",
            day: "numeric",
          });
        }
      }
    })
    .catch(function () {
      // Keep the fallback href; show a neutral label instead of a stale version.
      setVersion("최신 버전");
    });
})();
