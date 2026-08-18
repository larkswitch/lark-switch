import { useEffect, useState } from 'react';
import { Radio, RadioGroup, Switch, Tag, Toast, Typography } from '@douyinfe/semi-ui';
import { api } from '../api';
import { PageHeader } from '../components/PageHeader';
import { copy } from '../copy';
import { getThemeMode, setThemeMode, type ThemeMode } from '../theme';
import type { RuntimeIdentity, Snapshot } from '../types';
import { normalizeError } from '../utils';

const { Text } = Typography;

export interface SettingsPageProps {
  data: Snapshot;
  onReload: () => Promise<void>;
}

export function SettingsPage(props: SettingsPageProps) {
  const [autostart, setAutostartState] = useState<boolean | null>(null);
  const [saving, setSaving] = useState(false);
  const [pathBusy, setPathBusy] = useState(false);
  const [themeMode, setThemeModeState] = useState<ThemeMode>(() => getThemeMode());
  const [identity, setIdentity] = useState<RuntimeIdentity | null>(null);

  useEffect(() => {
    let active = true;
    void api.autostartStatus()
      .then((enabled) => {
        if (active) setAutostartState(enabled);
      })
      .catch((error) => Toast.error(normalizeError(error)));
    void api.runtimeIdentity()
      .then((value) => {
        if (active) setIdentity(value);
      })
      .catch((error) => Toast.error(normalizeError(error)));
    return () => { active = false; };
  }, []);

  const updateAutostart = async (enabled: boolean) => {
    try {
      setSaving(true);
      const actual = await api.setAutostart(enabled);
      setAutostartState(actual);
      Toast.success(actual ? copy.settings.autostartOn : copy.settings.autostartOff);
    } catch (error) {
      Toast.error(normalizeError(error));
    } finally {
      setSaving(false);
    }
  };

  const updatePathTakeover = async (enabled: boolean) => {
    try {
      setPathBusy(true);
      if (enabled) {
        await api.installPathTakeover();
        Toast.success(copy.settings.pathTakeoverOn);
      } else {
        await api.removePathTakeover();
        Toast.success(copy.settings.pathTakeoverOff);
      }
      await props.onReload();
    } catch (error) {
      Toast.error(normalizeError(error));
    } finally {
      setPathBusy(false);
    }
  };

  const updateThemeMode = (value: string | number) => {
    if (value !== 'system' && value !== 'light' && value !== 'dark') {
      return;
    }
    setThemeMode(value);
    setThemeModeState(value);
  };

  return (
    <section>
      <PageHeader
        title={copy.settings.title}
        description={copy.settings.description}
      />
      <div className="settings-list">
        <div className="settings-row">
          <div className="settings-copy">
            <Text strong>{copy.settings.autostart}</Text>
            <Text type="tertiary">
              {copy.settings.autostartHint}
            </Text>
          </div>
          <Switch
            aria-label={copy.settings.autostart}
            checked={Boolean(autostart)}
            loading={autostart === null || saving}
            disabled={autostart === null || saving}
            onChange={(checked) => void updateAutostart(checked)}
          />
        </div>
        <div className="settings-row">
          <div className="settings-copy">
            <Text strong>{copy.settings.pathTakeover}</Text>
            <Text type="tertiary">{copy.settings.pathTakeoverHint}</Text>
          </div>
          <Switch
            aria-label={copy.settings.pathTakeover}
            checked={props.data.settings.pathTakeoverEnabled}
            loading={pathBusy}
            disabled={pathBusy}
            onChange={(checked) => void updatePathTakeover(checked)}
          />
        </div>
        <div className="settings-row">
          <div className="settings-copy">
            <Text strong>{copy.settings.theme}</Text>
            <Text type="tertiary">{copy.settings.themeHint}</Text>
          </div>
          <RadioGroup
            type="button"
            className="settings-theme-radios"
            value={themeMode}
            aria-label={copy.settings.theme}
            onChange={(event) => updateThemeMode(event.target.value)}
          >
            <Radio value="system">{copy.settings.themeSystem}</Radio>
            <Radio value="light">{copy.settings.themeLight}</Radio>
            <Radio value="dark">{copy.settings.themeDark}</Radio>
          </RadioGroup>
        </div>
        <div className="settings-row settings-row-static">
          <div className="settings-copy">
            <Text strong>{copy.settings.closeWindow}</Text>
            <Text type="tertiary">{copy.settings.closeWindowHint}</Text>
          </div>
          <Tag color="green">{copy.settings.stayBackground}</Tag>
        </div>
        <div className="settings-row settings-row-static">
          <div className="settings-copy">
            <Text strong>{copy.settings.running}</Text>
            <Text type="tertiary" className="settings-identity-path">
              {identity
                ? `${copy.settings.runningVersion(identity.version)} · ${identity.exePath}`
                : copy.settings.runningHint}
            </Text>
          </div>
        </div>
      </div>
    </section>
  );
}
