// rubyscope web page: the header line, the screenshot demo, and the docs contents.

const head = document.querySelector(".site-head");
const onScroll = () => head.classList.toggle("scrolled", window.scrollY > 8);
window.addEventListener("scroll", onScroll, { passive: true });
onScroll();

// The demo shows one screenshot at a time. Every element with data-for
// belongs to one or more states; a click on data-go moves to that state.
const demo = document.getElementById("demo");
if (demo) {
  const show = (state, focus) => {
    demo.dataset.state = state;
    demo.querySelectorAll("[data-for]").forEach((el) => {
      el.hidden = !el.dataset.for.split(" ").includes(state);
    });
    demo.querySelector(".demo-reset").hidden = state === "start";
    if (focus) demo.querySelector(`.spot:not([hidden])`)?.focus();
  };
  demo.addEventListener("click", (event) => {
    const go = event.target.closest("[data-go]");
    if (go) show(go.dataset.go, event.detail === 0);
  });
  // The mouse back button returns, as in the app.
  demo.addEventListener("mouseup", (event) => {
    if (event.button === 3 && demo.dataset.state !== "start") show("start");
  });
}

// Docs: mark the section that is on screen in the contents.
const toc = document.querySelector(".toc");
if (toc) {
  const links = new Map([...toc.querySelectorAll("a")].map((a) => [a.hash.slice(1), a]));
  const seen = new Set();
  const observer = new IntersectionObserver(
    (entries) => {
      entries.forEach((e) => (e.isIntersecting ? seen.add(e.target.id) : seen.delete(e.target.id)));
      const first = [...links.keys()].find((id) => seen.has(id));
      if (!first) return;
      links.forEach((a, id) => a.classList.toggle("current", id === first));
    },
    { rootMargin: "-80px 0px -60% 0px" },
  );
  links.forEach((_, id) => {
    const section = document.getElementById(id);
    if (section) observer.observe(section);
  });
}
