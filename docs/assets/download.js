// Resolve the latest Windows installer from GitHub Releases and wire up the
// download button + version labels. Falls back to the releases/latest page if
// the API is unreachable (offline, rate-limited), so the button always works.
(function () {
  "use strict";

  var REPO = "raravel/eud-agent";
  var RELEASES_PAGE = "https://github.com/" + REPO + "/releases/latest";
  // List endpoint (not /releases/latest): the RAG index ships as its own
  // `rag-index-v*` release with no installer, and it can become GitHub's
  // "latest". Scan the list and pick the newest release that actually carries
  // the program installer, ignoring RAG-index-only releases.
  var API = "https://api.github.com/repos/" + REPO + "/releases?per_page=30";

  var btn = document.getElementById("download-btn");
  var versionEls = document.querySelectorAll("[data-version]");
  var dateEl = document.getElementById("pubdate");

  function setVersion(text) {
    for (var i = 0; i < versionEls.length; i++) {
      versionEls[i].textContent = text;
    }
  }

  // Pre-set the fallback so the button is functional before/without the API.
  if (btn) btn.href = RELEASES_PAGE;

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
      var installer = null;
      for (var r = 0; r < releases.length; r++) {
        if (releases[r].draft || releases[r].prerelease) continue;
        var assets = releases[r].assets || [];
        for (var i = 0; i < assets.length; i++) {
          var name = assets[i].name || "";
          if (/-setup\.exe$/i.test(name) && !/\.sig$/i.test(name)) {
            installer = assets[i];
            break;
          }
        }
        if (installer) {
          release = releases[r];
          break;
        }
      }

      if (!release) throw new Error("no installer release found");

      var tag = release.tag_name || "";
      if (tag) setVersion(tag);
      if (installer && btn) btn.href = installer.browser_download_url;

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
