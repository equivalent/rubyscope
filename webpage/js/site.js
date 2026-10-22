// rubyscope web page: the header line and the docs contents.

const head = document.querySelector(".site-head");
const onScroll = () => head.classList.toggle("scrolled", window.scrollY > 8);
window.addEventListener("scroll", onScroll, { passive: true });
onScroll();

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
