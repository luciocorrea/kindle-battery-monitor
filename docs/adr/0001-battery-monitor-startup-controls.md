# Separate automatic startup from the current monitor session

The KUAL battery monitor package keeps automatic startup independent of the current session: Enable/Disable change both, while Start/Stop change only the session, and upgrades preserve both settings. On the tested Kindle's Upstart 0.6.6, enabled and disabled startup use two recognized forms of the owned job, with the disabled form omitting its boot trigger; later `manual` and `.override` features are unavailable, and a persistent launch gate would add another runtime state source and startup wrapper.
