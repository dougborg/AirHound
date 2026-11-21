# Changelog

## [0.2.0](https://github.com/dougborg/AirHound/compare/airhound-v0.1.1...airhound-v0.2.0) (2026-11-21)


### Features

* add hostd — desktop WiFi/BLE scanner with WiGLE CSV output ([6d0aa28](https://github.com/dougborg/AirHound/commit/6d0aa280b668daa05fb169c3ef4fe5da285d139d))
* add low-band 5GHz reception and versioned capture validation ([25b32df](https://github.com/dougborg/AirHound/commit/25b32dfa1420adad9023aec0fdabd14281f8813d))
* add passive channel hopping and offline receiver diagnostics ([0046c87](https://github.com/dougborg/AirHound/commit/0046c87e0094961c32fa4d31913366c46a98043e))
* add standalone MT7610U userspace receiver experiment ([6f08cce](https://github.com/dougborg/AirHound/commit/6f08ccefa0f466070a02bedd21c4a4ce8bd6ef20))
* add WiFi security field and host board feature ([03dcbf5](https://github.com/dougborg/AirHound/commit/03dcbf5a5d5fba27c9c8f90967eb0b13af1e109b))
* attach bounded rule evidence and separate signature alert policy ([33aef4b](https://github.com/dougborg/AirHound/commit/33aef4b86ada633c1159d7c1df42d7a80cdecc07))
* **ble:** preserve bounded payload snapshots and full UUID identities ([94ba132](https://github.com/dougborg/AirHound/commit/94ba13284d3aa0b1f308205120a150cd781de3aa))
* **diagnostics:** locate scan queue losses within boot timeline ([c956d14](https://github.com/dougborg/AirHound/commit/c956d1418976c41020a468bc3d731e3e1666f2d0))
* **experiment:** add offline BLE HCI replay and receiver feasibility plan ([ce2316e](https://github.com/dougborg/AirHound/commit/ce2316efa6790121a6bcfd78f90cefca7a9ef084))
* **firmware:** stream ordinary observations with signature metadata ([dfa97e8](https://github.com/dougborg/AirHound/commit/dfa97e8ac1d7889eefca3aec5d9f5a1e229f3439))
* generate shared signature defaults from a canonical versioned pack ([5a2ea85](https://github.com/dougborg/AirHound/commit/5a2ea85f4dd037f91096091e20cc9f603fb0745b))
* **hostd:** capture Linux WiFi from monitor interfaces ([a850250](https://github.com/dougborg/AirHound/commit/a850250900d4068aa9361512165c8343ebeb7e51))
* **hostd:** configure scanners and protect CSV output ([25cfadd](https://github.com/dougborg/AirHound/commit/25cfadd06cec154c0dc183f432052af935ac0abe))
* **hostd:** decode typed BLE Remote ID service records ([2366ae2](https://github.com/dougborg/AirHound/commit/2366ae228dc7c32cd5fa93fab206dbc54f5b6302))
* **hostd:** geotag observations with optional gpsd input ([3eb8994](https://github.com/dougborg/AirHound/commit/3eb8994341abf7cf900a4e0b5bd679eecfcda1a4))
* **hostd:** report scanner health independently of scan requests ([2b6568f](https://github.com/dougborg/AirHound/commit/2b6568fd5c43471b76cf16d96f46353a7be8c560))
* **hostd:** route normalized BLE Remote ID with captured receiver provenance ([202279b](https://github.com/dougborg/AirHound/commit/202279bef6245af8d2876ee59fa655ca7d67dcfe))
* **hostd:** select BLE receivers and report capability evidence ([60132aa](https://github.com/dougborg/AirHound/commit/60132aa435afa986deece1f1d634a8fe6958fdf4))
* ingest live Sniffle and replayed BLE packets with shared evidence ([70f21f2](https://github.com/dougborg/AirHound/commit/70f21f208a738ba582186c4963e294198cf36eb4))
* **m5:** wire debounced page and sound buttons with on-screen hints ([3daec5c](https://github.com/dougborg/AirHound/commit/3daec5c70e4392911d500b698118968e841c20af))
* **receiver:** supervise passive streams and expand tested channel tables ([1ef17e6](https://github.com/dougborg/AirHound/commit/1ef17e685c50be236e63a2938402123400b5cd14))
* **release:** package desktop receivers firmware and source tools ([ac2c0af](https://github.com/dougborg/AirHound/commit/ac2c0afdfa64c9248f121acdea8cea238634d768))
* **signatures:** add sourced research candidates and admission checks ([0d9da0e](https://github.com/dougborg/AirHound/commit/0d9da0e2fa67f78c69d677afee1bf7c337f28382))


### Bug Fixes

* **ci:** select host toolchain when installing analysis tools ([bfbbb2b](https://github.com/dougborg/AirHound/commit/bfbbb2b5185495cf432ec36246bff553b79f8d37))
* **ci:** use desktop toolchain for public API build ([015f243](https://github.com/dougborg/AirHound/commit/015f2433ff275537b689c58f5a66884d346e3050))
* **deps:** patch std firmware dependency advisories ([47256a5](https://github.com/dougborg/AirHound/commit/47256a5192b9877d4a4e8696226b15c7a265af80))
* **firmware:** start WiFi and isolate observation storage in PSRAM ([8aa3aa3](https://github.com/dougborg/AirHound/commit/8aa3aa36fbeb11017bc708ec90ff1b68d911a888))
* gate radio capture on consumer and display readiness ([d4cb254](https://github.com/dougborg/AirHound/commit/d4cb2540ee1c6c5f0b4a878fa6c7349151f5e0d2))
* **hostd:** adapt security modes to wifi_scan 0.7.3 ([46ac29e](https://github.com/dougborg/AirHound/commit/46ac29e515d10f7c01a025bb5ec37e42b6c03342))
* **hostd:** declare Bluetooth usage in macOS app bundle ([6315410](https://github.com/dougborg/AirHound/commit/6315410be50a4b16ad55dbdba156efc9c3a3105b))
* **hostd:** preserve macOS BLE identity and control scanner lifecycle ([3a3f86e](https://github.com/dougborg/AirHound/commit/3a3f86ef95539eb4be66628985a52b790c5c9f0d))
* **hostd:** preserve unavailable Remote ID signal strength ([ff5342b](https://github.com/dougborg/AirHound/commit/ff5342b775ec835564baa1da962d079feeb73cec))
* **hostd:** resolve adversarial review findings ([db2cd1d](https://github.com/dougborg/AirHound/commit/db2cd1d43213543e6dd1589c7083d348646a5869))
* **hostd:** retain validated output and location integration fixes ([0f2fdb9](https://github.com/dougborg/AirHound/commit/0f2fdb972b564ee2b8cfc8755376cbda49de71c2))
* **hostd:** separate health records in benchmarks and scope permission state ([84d25c4](https://github.com/dougborg/AirHound/commit/84d25c418b61bc1b36ebccb6c99213c5901b2fce))
* keep legacy cached BLE evidence conservative ([b80aaa3](https://github.com/dougborg/AirHound/commit/b80aaa303b59d11984170d0f1346be9f0ad050d7))
* **m5:** reduce stream stalls and preserve startup stack margin ([ff855cc](https://github.com/dougborg/AirHound/commit/ff855cc2bd3049f0411a922370e505f7ccef6380))
* **m5:** yield to consumers after enabling radio capture ([c910a2d](https://github.com/dougborg/AirHound/commit/c910a2d80bba15348380ae7ce515bf90ef85d2d6))
* **odid:** preserve WiFi payloads and match BLE service data ([baa93c6](https://github.com/dougborg/AirHound/commit/baa93c6718f90350858c5bde6a02cd4c2c61d2f7))
* **odid:** read Remote ID framing as ASTM F3411 transmitters send it ([eaf95e3](https://github.com/dougborg/AirHound/commit/eaf95e3f91147d19015f84a6e60898ccf9d771c3))
* preserve queued replay on pause and drain live packets on shutdown ([f40fe99](https://github.com/dougborg/AirHound/commit/f40fe991fb2f5fe32b8a0ed51ba3707f629c675e))
* **protocol:** acknowledge command outcomes and reject unsupported actions ([55f17b5](https://github.com/dougborg/AirHound/commit/55f17b5d7355686dbfa347b4d553974a2df5d0d1))
* **proximity:** widen RSSI arithmetic before subtraction ([acc475c](https://github.com/dougborg/AirHound/commit/acc475c2cc2a80d829f9a1a5fb86531e83222f71))
* **receiver:** avoid name resolution during soak resource sampling ([bf94ad0](https://github.com/dougborg/AirHound/commit/bf94ad06c2439515f52a17785478cf0296c095d1))
* **receiver:** bound recovery after MCU command timeouts ([93ab3ee](https://github.com/dougborg/AirHound/commit/93ab3eee9aeb6c5c1a74352511006605543299e9))
* **receiver:** preserve firmware digest formatting with sha2 0.11 ([8ff5950](https://github.com/dougborg/AirHound/commit/8ff5950c7af2d4758d06022c032ddd826dd9a156))
* **release:** attach firmware before publishing, for immutable releases ([8a3b536](https://github.com/dougborg/AirHound/commit/8a3b5362b12e4918839bb57a3ffd5a1f3a7e021c))
* **release:** fit ESP-IDF applications in their actual partition layout ([86c1bc6](https://github.com/dougborg/AirHound/commit/86c1bc6dc82a34cfe3bbf72bf9bca54909d47aee))
* **release:** package merged images with checksums ([774a42e](https://github.com/dougborg/AirHound/commit/774a42e84ffdc9517db57a35aaef98df2d727435))
* **release:** synchronize dependent lockfiles and publish explicit previews ([78f225e](https://github.com/dougborg/AirHound/commit/78f225e81bad4ded99899478aeaca0bb9df6de62))
* **release:** use minor versions for pre-1.0 feature releases ([c0f2d57](https://github.com/dougborg/AirHound/commit/c0f2d57243d219dd301e8a77bd01b458a502a7a3))
* **release:** use supported macOS runner and document package entry points ([c3a50c3](https://github.com/dougborg/AirHound/commit/c3a50c3194f0127a21ca85a5e0be53f1c9a20b8f))
* report stopped and paused packet receiver availability ([b0ee5dd](https://github.com/dougborg/AirHound/commit/b0ee5dd99d48a05b3b74177a6ee87a3e7e6f1e83))
* require regular capture files for bounded replay startup ([07ebd62](https://github.com/dougborg/AirHound/commit/07ebd62342cc41105ed2c69d58fd2add4a772ab9))
* validate standalone evidence schemas offline ([7ec4ad3](https://github.com/dougborg/AirHound/commit/7ec4ad33065a23e8b1f10a523e6d490573b06ba9))

## [0.1.1](https://github.com/dougborg/AirHound/compare/airhound-v0.1.0...airhound-v0.1.1) (2026-07-25)


### Features

* add boolean rule composition engine and ble_ad_bytes matching ([251d0c2](https://github.com/dougborg/AirHound/commit/251d0c20d45c1658067cd39ac63f48ad63329cc7))
* add CI/CD pipeline with testable library crate ([#6](https://github.com/dougborg/AirHound/issues/6)) ([2716039](https://github.com/dougborg/AirHound/commit/271603909d858604e990125410cdc595d57e982b))
* add ESP-IDF std firmware for A/B comparison ([8e0c135](https://github.com/dougborg/AirHound/commit/8e0c1354914d05ed910b1fb52a36cbb18cf7ae1f))
* add MAC index, new signatures, and filter pipeline integration ([c52846b](https://github.com/dougborg/AirHound/commit/c52846b4664dd43aa50180d4bf98c01df70344d9))
* add MatchCategory routing to rule engine ([#70](https://github.com/dougborg/AirHound/issues/70)) ([840e1ae](https://github.com/dougborg/AirHound/commit/840e1aeffb006e8b5f77b69407aa57cfbfbd91ca))
* add new host commands for mode, proximity, watchlist, category ([#72](https://github.com/dougborg/AirHound/issues/72)) ([9abbf67](https://github.com/dougborg/AirHound/commit/9abbf676465828f37b2976fca61ca18e384a546c))
* add Open Drone ID parser (odid.rs) ([e99c942](https://github.com/dougborg/AirHound/commit/e99c942d21b80706c667730b8b778a849bd0119f)), closes [#66](https://github.com/dougborg/AirHound/issues/66)
* add RSSI proximity logic module ([#68](https://github.com/dougborg/AirHound/issues/68)) ([bd703c8](https://github.com/dougborg/AirHound/commit/bd703c887d71e0b79b579c22b0362e606fde1ae0))
* add ScanEvent::Odid variant to scanner ([#69](https://github.com/dougborg/AirHound/issues/69)) ([793f778](https://github.com/dougborg/AirHound/commit/793f77899f826abb004661b7b2cc25fe4df3f1ba))
* add watchlist matching module ([#67](https://github.com/dougborg/AirHound/issues/67)) ([0495cd0](https://github.com/dougborg/AirHound/commit/0495cd0a21034e673f6e876b364b05d9001c8098))
* enable buzzer support on both XIAO and M5StickC boards ([a145056](https://github.com/dougborg/AirHound/commit/a1450564bcee70f13d362c24a0078f3ab6d5db0a))
* extend NDJSON protocol with drone, proximity, alert messages ([#71](https://github.com/dougborg/AirHound/issues/71)) ([177552a](https://github.com/dougborg/AirHound/commit/177552a38baabc5684dc2f62cf4d6bb1f8597ddb))
* wire ODID parser into firmware pipeline ([95c3942](https://github.com/dougborg/AirHound/commit/95c3942992485c65a4a3e5b824480811b3f410f4))


### Bug Fixes

* address code review findings in ODID parser ([a3a6442](https://github.com/dougborg/AirHound/commit/a3a6442f25a954c3c28b758f8703d39a80d1e160))
* address PR review comments and add std preview artifacts ([add2b93](https://github.com/dougborg/AirHound/commit/add2b93b30e1e25872009b4e8041d2ff4be87c03))
* address PR review feedback ([54a043f](https://github.com/dougborg/AirHound/commit/54a043f3b9e3d67ef40e8fcc46984a00474f62f6))
* address PR review feedback on preview-comment script ([f183dff](https://github.com/dougborg/AirHound/commit/f183dffd56dff83b5709be0071dd33e64631109b))
* address second round of code review findings ([3d5612c](https://github.com/dougborg/AirHound/commit/3d5612c8953dae1916816fba905a164374eb7e14))
* guard buzzer module and spawn behind board feature cfg ([4d71d45](https://github.com/dougborg/AirHound/commit/4d71d454ad781d46820aa6f38088c81d2262880c))
* initialize LEDC slow clock and add boot beep on no_std buzzer ([e436a78](https://github.com/dougborg/AirHound/commit/e436a78e76aa0e4d5a05b87844c8c196189607a5))
* scope linker flags to no_std targets only ([66c6b21](https://github.com/dougborg/AirHound/commit/66c6b21dfcddcf7f47ba7f1deda2899500ac3806))
* update firmware-std for rule engine and ble_ad_bytes fields ([915d27c](https://github.com/dougborg/AirHound/commit/915d27ce31e6a2eb170f414deb336729605235f4))
