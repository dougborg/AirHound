# Changelog

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
