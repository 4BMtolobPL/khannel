# COPR "make_srpm" 빌드 방식: COPR가 저장소를 체크아웃한 뒤 mock 안에서(네트워크 허용)
#   make -f .copr/Makefile srpm outdir=<출력> spec=<spec>
# 를 실행한다. 의존성 vendoring까지 여기서 하므로 빌드 단계는 오프라인이다.
srpm:
	dnf -y install git cargo cargo-vendor-filterer rpm-build
	git config --global --add safe.directory "$(CURDIR)"
	dist/rpm/make-srpm.sh $(outdir)
