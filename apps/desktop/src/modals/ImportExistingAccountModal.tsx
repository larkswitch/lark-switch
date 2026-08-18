import { useEffect, useRef, useState } from 'react';
import { Banner, Button, Card, Input, Modal, Space, Spin, Tag, Toast, Typography } from '@douyinfe/semi-ui';
import { IconRefresh } from '@douyinfe/semi-icons';
import { api } from '../api';
import { copy } from '../copy';
import type { ExistingCliCandidate } from '../types';
import { maskId, normalizeError } from '../utils';

const { Text } = Typography;

export type DiscoveryStatus = 'idle' | 'scanning' | 'success' | 'error';

export interface ImportExistingAccountModalProps {
  visible: boolean;
  initialCandidates: ExistingCliCandidate[];
  initialDiscoveryStatus: DiscoveryStatus;
  initialDiscoveryError: string;
  onClose: () => void;
  onDone: () => Promise<void>;
}

export function ImportExistingAccountModal(props: ImportExistingAccountModalProps) {
  const [candidates, setCandidates] = useState<ExistingCliCandidate[]>(props.initialCandidates);
  const [configDir, setConfigDir] = useState('');
  const [label, setLabel] = useState('');
  const [busy, setBusy] = useState(false);
  const [scanStatus, setScanStatus] = useState<DiscoveryStatus>('idle');
  const [scanError, setScanError] = useState('');
  const scanRequestId = useRef(0);
  const activeScanRequestId = useRef<number | null>(null);

  const selectCandidate = (candidate: ExistingCliCandidate) => {
    setConfigDir(candidate.configDir);
    setLabel(candidate.brand === 'feishu' ? copy.importAccount.localFeishu : copy.importAccount.localLark);
  };

  const scan = async () => {
    if (activeScanRequestId.current !== null) return;
    const requestId = ++scanRequestId.current;
    activeScanRequestId.current = requestId;
    setScanStatus('scanning');
    setScanError('');
    try {
      const found = await api.discoverExistingConfigs();
      if (requestId !== scanRequestId.current) return;
      setCandidates(found);
      if (found.length > 0) {
        setConfigDir((current) => current || found[0].configDir);
        setLabel((current) => current || (found[0].brand === 'feishu' ? copy.importAccount.localFeishu : copy.importAccount.localLark));
      }
      setScanStatus('success');
    } catch (error) {
      if (requestId !== scanRequestId.current) return;
      setScanError(normalizeError(error));
      setScanStatus('error');
    } finally {
      if (activeScanRequestId.current === requestId) {
        activeScanRequestId.current = null;
      }
    }
  };

  useEffect(() => {
    scanRequestId.current += 1;
    activeScanRequestId.current = null;
    if (!props.visible) {
      setScanStatus('idle');
      setScanError('');
      return;
    }

    setCandidates(props.initialCandidates);
    setScanStatus(props.initialDiscoveryStatus);
    setScanError(props.initialDiscoveryError);
    if (props.initialCandidates.length > 0) {
      setConfigDir((current) => current || props.initialCandidates[0].configDir);
      setLabel((current) => current || (
        props.initialCandidates[0].brand === 'feishu' ? copy.importAccount.localFeishu : copy.importAccount.localLark
      ));
    }
  }, [
    props.visible,
    props.initialCandidates,
    props.initialDiscoveryStatus,
    props.initialDiscoveryError,
  ]);

  const closeModal = () => {
    scanRequestId.current += 1;
    activeScanRequestId.current = null;
    props.onClose();
  };

  const submit = async () => {
    try {
      setBusy(true);
      const result = await api.importExistingAccountConfig(label.trim(), configDir.trim());
      Toast.success(result.alreadyImported ? copy.importAccount.alreadyInProduct : copy.importAccount.imported(result.account.displayName));
      closeModal();
      await props.onDone();
    } catch (error) {
      Toast.error(normalizeError(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title={copy.importAccount.title}
      visible={props.visible}
      onCancel={closeModal}
      className="lpc-modal"
      modalContentClass="lpc-modal-content"
      footer={(
        <>
          <Button disabled={busy} onClick={closeModal}>{copy.common.cancel}</Button>
          <Button
            theme="solid"
            loading={busy}
            disabled={!configDir.trim() || !label.trim()}
            onClick={() => void submit()}
          >
            {copy.importAccount.submit}
          </Button>
        </>
      )}
      width={620}
    >
      <div className="modal-stack">
        <Banner
          type="info"
          description={copy.importAccount.banner}
        />
        <div className="form-stack">
          <div className="card-actions">
            <Text strong>{copy.importAccount.defaultLocation}</Text>
            <Button
              loading={scanStatus === 'scanning'}
              disabled={scanStatus === 'scanning'}
              icon={<IconRefresh />}
              onClick={() => void scan()}
            >
              {copy.importAccount.rescan}
            </Button>
          </div>
          {scanStatus === 'error' ? (
            <Banner type="danger" description={copy.importAccount.scanFailed(scanError)} />
          ) : scanStatus === 'scanning' && candidates.length === 0 ? (
            <Space>
              <Spin size="small" />
              <Text type="tertiary">{copy.importAccount.scanning}</Text>
            </Space>
          ) : scanStatus === 'success' && candidates.length === 0 ? (
            <Text type="tertiary">{copy.importAccount.noneFound}</Text>
          ) : scanStatus === 'idle' && candidates.length === 0 ? (
            <Text type="tertiary">{copy.importAccount.idleHint}</Text>
          ) : (
            candidates.map((candidate) => (
              <Card
                key={candidate.configDir}
                className={`migration-candidate-card ${
                  configDir === candidate.configDir ? 'migration-candidate-card-selected' : ''
                }`}
              >
                <div className="card-title-row">
                  <div className="candidate-card-copy">
                    <Text strong>{candidate.displayName}</Text>
                    <div>
                      <Text type="tertiary">
                        {candidate.brand === 'feishu' ? copy.importAccount.brandFeishu : copy.importAccount.brandLark} · {maskId(candidate.appId)} · {maskId(candidate.userOpenId)}
                      </Text>
                    </div>
                  </div>
                  <Tag color={candidate.health === 'ready' ? 'green' : 'orange'}>
                    {candidate.alreadyImported
                      ? copy.importAccount.alreadyImported
                      : candidate.health === 'ready'
                        ? copy.importAccount.importable
                        : copy.importAccount.needsCheck}
                  </Tag>
                  <Button onClick={() => selectCandidate(candidate)}>{copy.importAccount.select}</Button>
                </div>
              </Card>
            ))
          )}
          <label>
            {copy.importAccount.configDir}
            <Input
              value={configDir}
              onChange={setConfigDir}
              placeholder={copy.importAccount.configPlaceholder}
            />
          </label>
          <label>
            {copy.importAccount.label}
            <Input value={label} onChange={setLabel} placeholder={copy.importAccount.labelPlaceholder} />
          </label>
        </div>
      </div>
    </Modal>
  );
}
