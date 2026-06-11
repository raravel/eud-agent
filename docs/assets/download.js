// Resolve the latest Windows installer from GitHub Releases and wire up the
// download button + version labels. Falls back to the releases/latest page if
// the API is unreachable (offline, rate-limited), so the button always works.
(function () {
  "use strict";

  var REPO = "raravel/eud-agent";
  var RELEASES_PAGE = "https://github.com/" + REPO + "/releases/latest";
  var API = "https://api.github.com/repos/" + REPO + "/releases/latest";

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
    .then(function (data) {
      var tag = data.tag_name || "";
      var assets = data.assets || [];
      var installer = null;
      for (var i = 0; i < assets.length; i++) {
        var name = assets[i].name || "";
        if (/-setup\.exe$/i.test(name) && !/\.sig$/i.test(name)) {
          installer = assets[i];
          break;
        }
      }

      if (tag) setVersion(tag);
      if (installer && btn) btn.href = installer.browser_download_url;

      if (data.published_at && dateEl) {
        var d = new Date(data.published_at);
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
