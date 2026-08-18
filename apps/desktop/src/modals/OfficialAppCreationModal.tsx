import { useEffect, useRef, useState } from 'react';
import { Banner, Button, Input, Modal, Select, Space, Spin, Toast, Typography } from '@douyinfe/semi-ui';
import { openUrl } from '@tauri-apps/plugin-opener';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import { api } from '../api';
import { copy } from '../copy';
import type { AppCreationStart, Brand } from '../types';
import { normalizeError } from '../utils';

const { Text } = Typography;

export interface OfficialAppCreationModalProps {
  visible: boolean;
  onClose: () => void;
  onDone: () => Promise<void>;
}

export function OfficialAppCreationModal(props: OfficialAppCreationModalProps) {
  const [label, setLabel] = useState('');
  const [brand, setBrand] = useState<Brand>('feishu');
  const [flow, setFlow] = useState<AppCreationStart | null>(null);
  const [busy, setBusy] = useState(false);
  const [polling, setPolling] = useState(false);
  const [error, setError] = useState('');
  const pollInFlight = useRef(false);

  const reset = () => {
    setFlow(null);
    setBusy(false);
    setPolling(false);
    setError('');
  };

  const close = async () => {
    if (pollInFlight.current) return;
    pollInFlight.current = true;
    const flowId = flow?.flowId;
    setBusy(true);
    try {
      if (flowId) {
        await api.cancelOfficialAppCreation(flowId);
      }
    } catch (cancelError) {
      Toast.error(normalizeError(cancelError));
      setBusy(false);
      return;
    } finally {
      pollInFlight.current = false;
    }
    reset();
    props.onClose();
  };

  useEffect(() => {
    const flowId = flow?.flowId;
    if (!props.visible || !flowId || error) return;

    let active = true;
    const poll = async () => {
      if (pollInFlight.current) return;
      pollInFlight.current = true;
      setPolling(true);
      try {
        const progress = await api.pollOfficialAppCreation(flowId);
        if (!active || !progress.complete) return;
        reset();
        props.onClose();
        await props.onDone();
        Toast.success(copy.createApp.done);
      } catch (pollError) {
        if (!active) return;
        try {
          await api.cancelOfficialAppCreation(flowId);
        } catch {
          // A failed poll may already have removed and cleaned the backend flow.
        }
        setFlow(null);
        setError(normalizeError(pollError));
      } finally {
        pollInFlight.current = false;
        if (active) setPolling(false);
      }
    };
    const timer = window.setInterval(() => void poll(), 2000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [error, flow?.flowId, props.visible]);

  const begin = async () => {
    try {
      setBusy(true);
      setError('');
      const started = await api.beginOfficialAppCreation(label.trim(), brand);
      setFlow(started);
    } catch (beginError) {
      setError(normalizeError(beginError));
    } finally {
      setBusy(false);
    }
  };

  const copyUrl = async () => {
    if (!flow) return;
    try {
      await writeText(flow.verificationUrl);
      Toast.success(copy.createApp.copied);
    } catch (copyError) {
      Toast.error(normalizeError(copyError));
    }
  };

  const openCreationUrl = async () => {
    if (!flow) return;
    try {
      await openUrl(flow.verificationUrl);
    } catch (openError) {
      Toast.error(normalizeError(openError));
    }
  };

  return (
    <Modal
      title={copy.createApp.title}
      visible={props.visible}
      onCancel={() => void close()}
      className="lpc-modal"
      modalContentClass="lpc-modal-content"
      footer={!flow ? (
        <>
          <Button disabled={busy || polling} onClick={() => void close()}>{copy.common.cancel}</Button>
          <Button
            theme="solid"
            loading={busy}
            disabled={!label.trim()}
            onClick={() => void begin()}
          >
            {copy.createApp.getLink}
          </Button>
        </>
      ) : (
        <Button disabled={busy || polling} onClick={() => void close()}>
          {copy.createApp.cancelCreate}
        </Button>
      )}
      width={620}
      closable={!busy && !polling}
      closeOnEsc={!busy && !polling}
      maskClosable={!busy && !polling}
    >
      <div className="app-creation-panel">
        {!flow ? (
          <>
            <Banner
              type="info"
              description={copy.createApp.banner}
            />
            {error && <Banner type="danger" description={error} />}
            <div className="form-stack">
              <label>
                {copy.createApp.label}
                <Input value={label} onChange={setLabel} placeholder={copy.createApp.labelPlaceholder} />
              </label>
              <label>
                <span id="create-app-brand-label">{copy.createApp.brand}</span>
                <Select
                  aria-labelledby="create-app-brand-label"
                  value={brand}
                  onChange={(value) => setBrand(String(value) as Brand)}
                  inputProps={{ 'aria-label': '创建 App 品牌' }}
                  optionList={[
                    { value: 'feishu', label: copy.createApp.feishu },
                    { value: 'lark', label: copy.createApp.lark },
                  ]}
                />
              </label>
            </div>
          </>
        ) : (
          <>
            <Banner
              type="warning"
              description={copy.createApp.waitingBanner}
            />
            <label className="auth-url-field">
              {copy.createApp.urlLabel}
              <textarea
                aria-label={copy.createApp.urlLabel}
                readOnly
                rows={4}
                value={flow.verificationUrl}
              />
            </label>
            <div className="auth-url-actions">
              <Button onClick={() => void copyUrl()}>{copy.createApp.copyLink}</Button>
              <Button theme="solid" onClick={() => void openCreationUrl()}>
                {copy.createApp.openBrowser}
              </Button>
            </div>
            <Space>
              <Spin size="small" />
              <Text type="tertiary">{copy.createApp.polling}</Text>
            </Space>
          </>
        )}
      </div>
    </Modal>
  );
}
