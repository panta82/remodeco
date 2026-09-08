Name: remodeco
Version: %{release_version}
Release: 1
Summary: Review and execute file renames in your editor
License: MIT
URL: https://github.com/panta82/remodeco
Source0: remodeco
Source1: LICENSE
Source2: README.md
Source3: ARCHITECTURE.md
# The packager verifies this is static; RPM otherwise invents an rtld dependency for PIE.
AutoReqProv: no

# Cargo already strips this static executable. Keep packaging from modifying it.
%global debug_package %{nil}
%global __os_install_post %{nil}
%global _build_id_links none

%description
Full-screen terminal preview for file moves, copies, and desktop trash,
with persistent sessions and undo journals.

%prep
%build
%install
install -D -m 0755 %{SOURCE0} %{buildroot}%{_bindir}/remodeco
install -D -m 0644 %{SOURCE1} %{buildroot}%{_licensedir}/remodeco/LICENSE
install -D -m 0644 %{SOURCE2} %{buildroot}%{_docdir}/remodeco/README.md
install -D -m 0644 %{SOURCE3} %{buildroot}%{_docdir}/remodeco/ARCHITECTURE.md

%files
%{_bindir}/remodeco
%license %{_licensedir}/remodeco/LICENSE
%doc %{_docdir}/remodeco/README.md
%doc %{_docdir}/remodeco/ARCHITECTURE.md
